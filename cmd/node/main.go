package main

import (
	"flag"
	"fmt"
	"io"
	"log"
	"net"
	"os"
	"os/signal"
	"sync"
	"syscall"

	"github.com/google/uuid"
	"github.com/tri/orb/node"
)

// Bridge represents an active connection to a service
type Bridge struct {
	ID       string
	Service  node.Service
	Conn     net.Conn
	StopChan chan struct{}
}

// NodeClient manages the connection to the relay server
type NodeClient struct {
	relayAddr string
	nodeID    string
	allowlist *node.AllowList
	conn      *node.Conn
	bridges   map[string]*Bridge
	bridgeMu  sync.RWMutex
	shutdown  chan struct{}
	closed    bool
	closeMu   sync.Mutex
}

func main() {
	relayAddr := flag.String("relay", "127.0.0.1:9000", "Relay server address")
	nodeID := flag.String("id", "", "Node ID (auto-generated if empty)")
	configPath := flag.String("config", "allowlist.json", "Path to allowlist config")
	flag.Parse()

	// Generate node ID if not provided
	if *nodeID == "" {
		*nodeID = "node-" + uuid.New().String()[:8]
	}

	// Load allowlist
	allowlist, err := node.LoadAllowList(*configPath)
	if err != nil {
		log.Fatalf("Failed to load allowlist: %v", err)
	}

	if len(allowlist.Services) == 0 {
		log.Fatalf("No services defined in allowlist")
	}

	log.Printf("[Node] Starting with ID: %s", *nodeID)
	for _, svc := range allowlist.Services {
		log.Printf("[Node] Service: %s -> %s:%d%s", svc.ID, svc.Addr, svc.Port, svc.Path)
	}

	// Create node client
	client := &NodeClient{
		relayAddr: *relayAddr,
		nodeID:    *nodeID,
		allowlist: allowlist,
		bridges:   make(map[string]*Bridge),
		shutdown:  make(chan struct{}),
	}

	// Handle shutdown signals
	sigChan := make(chan os.Signal, 1)
	signal.Notify(sigChan, syscall.SIGINT, syscall.SIGTERM)

	go func() {
		<-sigChan
		log.Println("[Node] Shutting down...")
		client.Close()
		os.Exit(0)
	}()

	// Connect and run
	if err := client.Run(); err != nil {
		log.Fatalf("[Node] Error: %v", err)
	}
}

// Run connects to the relay and handles messages
func (nc *NodeClient) Run() error {
	// Connect to relay
	conn, err := net.Dial("tcp", nc.relayAddr)
	if err != nil {
		return fmt.Errorf("dial relay: %w", err)
	}
	log.Printf("[Node] Connected to relay at %s", nc.relayAddr)

	nc.conn = node.NewConn(conn)

	// Send REGISTER
	if err := nc.sendRegister(); err != nil {
		return fmt.Errorf("send register: %w", err)
	}

	// Send ANNOUNCE
	if err := nc.sendAnnounce(); err != nil {
		return fmt.Errorf("send announce: %w", err)
	}

	// Message loop
	return nc.messageLoop()
}

// sendRegister sends a REGISTER message to the relay
func (nc *NodeClient) sendRegister() error {
	msgID := uuid.New().String()
	msg := node.NewRegisterMsg(msgID, nc.nodeID)
	if err := nc.conn.WriteMessage(msg); err != nil {
		return fmt.Errorf("write register: %w", err)
	}
	log.Printf("[Node] Sent REGISTER (node_id=%s)", nc.nodeID)
	return nil
}

// sendAnnounce sends an ANNOUNCE message with available services
func (nc *NodeClient) sendAnnounce() error {
	msgID := uuid.New().String()
	msg := node.NewAnnounceMsg(msgID, nc.allowlist.Services)
	if err := nc.conn.WriteMessage(msg); err != nil {
		return fmt.Errorf("write announce: %w", err)
	}
	log.Printf("[Node] Sent ANNOUNCE (%d services)", len(nc.allowlist.Services))
	return nil
}

// messageLoop handles incoming messages from the relay
func (nc *NodeClient) messageLoop() error {
	for {
		select {
		case <-nc.shutdown:
			return nil
		default:
		}

		msg, err := nc.conn.ReadMessage()
		if err != nil {
			return fmt.Errorf("read message: %w", err)
		}

		if err := nc.handleMessage(msg); err != nil {
			log.Printf("[Node] Error handling message: %v", err)
		}
	}
}

// handleMessage dispatches a message to the appropriate handler
func (nc *NodeClient) handleMessage(msg *node.Message) error {
	if msg.IsControl() {
		return nc.handleControl(msg)
	}
	if msg.IsData() {
		return nc.handleData(msg)
	}
	return fmt.Errorf("unknown message type")
}

// handleControl handles control messages from the relay
func (nc *NodeClient) handleControl(msg *node.Message) error {
	ctrl := msg.Control

	switch ctrl.Type {
	case node.MsgTypeAck:
		log.Printf("[Node] Received ACK for %s", ctrl.AckMsgID)
		return nil

	case node.MsgTypeOpenBridge:
		return nc.handleOpenBridge(msg)

	case node.MsgTypeCloseBridge:
		return nc.handleCloseBridge(ctrl.BridgeID)

	default:
		return fmt.Errorf("unknown control type: %s", ctrl.Type)
	}
}

// handleOpenBridge handles OPEN_BRIDGE requests from the relay
func (nc *NodeClient) handleOpenBridge(msg *node.Message) error {
	ctrl := msg.Control
	bridgeID := ctrl.BridgeID
	service := ctrl.Service

	if service == nil {
		return fmt.Errorf("open_bridge: missing service")
	}

	log.Printf("[Node] OPEN_BRIDGE %s -> %s:%d%s", bridgeID, service.Addr, service.Port, service.Path)

	// Connect to the service
	serviceAddr := fmt.Sprintf("%s:%d", service.Addr, service.Port)
	svcConn, err := net.Dial("tcp", serviceAddr)
	if err != nil {
		// Send ACK to acknowledge the request (even if it failed)
		nc.sendAck(msg.MsgID)
		return fmt.Errorf("dial service %s: %w", serviceAddr, err)
	}

	log.Printf("[Node] Connected to service %s", serviceAddr)

	// Create bridge
	bridge := &Bridge{
		ID:       bridgeID,
		Service:  *service,
		Conn:     svcConn,
		StopChan: make(chan struct{}),
	}

	nc.bridgeMu.Lock()
	nc.bridges[bridgeID] = bridge
	nc.bridgeMu.Unlock()

	// Send ACK
	if err := nc.sendAck(msg.MsgID); err != nil {
		bridge.Close()
		delete(nc.bridges, bridgeID)
		return fmt.Errorf("send ack: %w", err)
	}

	// Start bidirectional forwarding
	go nc.forwardServiceToRelay(bridge)
	go nc.forwardRelayToService(bridge)

	return nil
}

// handleCloseBridge handles CLOSE_BRIDGE requests from the relay
func (nc *NodeClient) handleCloseBridge(bridgeID string) error {
	log.Printf("[Node] CLOSE_BRIDGE %s", bridgeID)

	nc.bridgeMu.Lock()
	bridge, exists := nc.bridges[bridgeID]
	if exists {
		delete(nc.bridges, bridgeID)
	}
	nc.bridgeMu.Unlock()

	if exists {
		bridge.Close()
	}

	return nil
}

// handleData handles DATA messages from the relay (to forward to service)
func (nc *NodeClient) handleData(msg *node.Message) error {
	data := msg.Data

	nc.bridgeMu.RLock()
	bridge, exists := nc.bridges[data.BridgeID]
	nc.bridgeMu.RUnlock()

	if !exists {
		return fmt.Errorf("data for unknown bridge: %s", data.BridgeID)
	}

	// Forward payload to service
	_, err := bridge.Conn.Write(data.Payload)
	if err != nil {
		return fmt.Errorf("write to service: %w", err)
	}

	return nil
}

// forwardServiceToRelay reads from service and sends to relay as DataMsg
func (nc *NodeClient) forwardServiceToRelay(bridge *Bridge) {
	buf := make([]byte, 4096)
	for {
		select {
		case <-bridge.StopChan:
			return
		case <-nc.shutdown:
			return
		default:
		}

		n, err := bridge.Conn.Read(buf)
		if err != nil {
			if err != io.EOF {
				log.Printf("[Node] Bridge %s: read error: %v", bridge.ID, err)
			}
			return
		}

		if n > 0 {
			payload := make([]byte, n)
			copy(payload, buf[:n])

			msgID := uuid.New().String()
			msg := node.NewDataMsg(msgID, bridge.ID, payload)

			if err := nc.conn.WriteMessage(msg); err != nil {
				log.Printf("[Node] Bridge %s: write to relay error: %v", bridge.ID, err)
				return
			}
		}
	}
}

// forwardRelayToService is a placeholder - data is forwarded via handleData
// This goroutine just monitors for shutdown
func (nc *NodeClient) forwardRelayToService(bridge *Bridge) {
	<-bridge.StopChan
}

// sendAck sends an ACK message to the relay
func (nc *NodeClient) sendAck(ackMsgID string) error {
	msgID := uuid.New().String()
	msg := node.NewAckMsg(msgID, ackMsgID)
	return nc.conn.WriteMessage(msg)
}

// Close shuts down the node client
func (nc *NodeClient) Close() {
	nc.closeMu.Lock()
	defer nc.closeMu.Unlock()

	if nc.closed {
		return
	}
	nc.closed = true

	close(nc.shutdown)

	// Close all bridges
	nc.bridgeMu.Lock()
	for _, bridge := range nc.bridges {
		bridge.Close()
	}
	nc.bridges = make(map[string]*Bridge)
	nc.bridgeMu.Unlock()

	// Close relay connection
	if nc.conn != nil {
		nc.conn.Close()
	}
}

// Close closes the bridge and its connection
func (b *Bridge) Close() {
	close(b.StopChan)
	if b.Conn != nil {
		b.Conn.Close()
	}
}
