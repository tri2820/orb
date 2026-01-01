package node

import (
	"fmt"
	"io"
	"log"
	"net"
	"sync"

	"github.com/google/uuid"
)

// Bridge represents an active connection to a service
type Bridge struct {
	ID       string
	Service  Service
	Conn     net.Conn
	StopChan chan struct{}
}

// NodeClient manages the connection to the relay server
type NodeClient struct {
	relayAddr string
	nodeID    string
	services  []Service
	conn      *Conn
	bridges   map[string]*Bridge
	bridgeMu  sync.RWMutex
	shutdown  chan struct{}
	closed    bool
	closeMu   sync.Mutex

	// For testing: track received data per bridge
	DataReceived map[string][]byte
	dataMu       sync.Mutex
}

// NewNodeClient creates a new node client
func NewNodeClient(relayAddr, nodeID string, services []Service) *NodeClient {
	return &NodeClient{
		relayAddr:    relayAddr,
		nodeID:       nodeID,
		services:     services,
		bridges:      make(map[string]*Bridge),
		shutdown:     make(chan struct{}),
		DataReceived: make(map[string][]byte),
	}
}

// Run connects to the relay and handles messages
func (nc *NodeClient) Run() error {
	// Connect to relay
	conn, err := net.Dial("tcp", nc.relayAddr)
	if err != nil {
		return fmt.Errorf("dial relay: %w", err)
	}

	nc.conn = NewConn(conn)

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
	msg := NewRegisterMsg(msgID, nc.nodeID)
	if err := nc.conn.WriteMessage(msg); err != nil {
		return fmt.Errorf("write register: %w", err)
	}
	return nil
}

// sendAnnounce sends an ANNOUNCE message with available services
func (nc *NodeClient) sendAnnounce() error {
	msgID := uuid.New().String()
	msg := NewAnnounceMsg(msgID, nc.services)
	if err := nc.conn.WriteMessage(msg); err != nil {
		return fmt.Errorf("write announce: %w", err)
	}
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
func (nc *NodeClient) handleMessage(msg *Message) error {
	if msg.IsControl() {
		return nc.handleControl(msg)
	}
	if msg.IsData() {
		return nc.handleData(msg)
	}
	return fmt.Errorf("unknown message type")
}

// handleControl handles control messages from the relay
func (nc *NodeClient) handleControl(msg *Message) error {
	ctrl := msg.Control

	switch ctrl.Type {
	case MsgTypeAck:
		return nil

	case MsgTypeOpenBridge:
		return nc.handleOpenBridge(msg)

	case MsgTypeCloseBridge:
		return nc.handleCloseBridge(ctrl.BridgeID)

	default:
		return fmt.Errorf("unknown control type: %s", ctrl.Type)
	}
}

// handleOpenBridge handles OPEN_BRIDGE requests from the relay
func (nc *NodeClient) handleOpenBridge(msg *Message) error {
	ctrl := msg.Control
	bridgeID := ctrl.BridgeID
	service := ctrl.Service

	if service == nil {
		return fmt.Errorf("open_bridge: missing service")
	}

	// Connect to the service
	serviceAddr := fmt.Sprintf("%s:%d", service.Addr, service.Port)
	svcConn, err := net.Dial("tcp", serviceAddr)
	if err != nil {
		nc.sendAck(msg.MsgID)
		return fmt.Errorf("dial service %s: %w", serviceAddr, err)
	}

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

	return nil
}

// handleCloseBridge handles CLOSE_BRIDGE requests from the relay
func (nc *NodeClient) handleCloseBridge(bridgeID string) error {
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
func (nc *NodeClient) handleData(msg *Message) error {
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
			// Store for testing
			nc.dataMu.Lock()
			nc.DataReceived[bridge.ID] = append(nc.DataReceived[bridge.ID], buf[:n]...)
			nc.dataMu.Unlock()

			msgID := uuid.New().String()
			msg := NewDataMsg(msgID, bridge.ID, buf[:n])

			if err := nc.conn.WriteMessage(msg); err != nil {
				log.Printf("[Node] Bridge %s: write to relay error: %v", bridge.ID, err)
				return
			}
		}
	}
}

// sendAck sends an ACK message to the relay
func (nc *NodeClient) sendAck(ackMsgID string) error {
	msgID := uuid.New().String()
	msg := NewAckMsg(msgID, ackMsgID)
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

// GetReceivedData returns data received for a specific bridge (for testing)
func (nc *NodeClient) GetReceivedData(bridgeID string) []byte {
	nc.dataMu.Lock()
	defer nc.dataMu.Unlock()
	return nc.DataReceived[bridgeID]
}

// Close closes the bridge and its connection
func (b *Bridge) Close() {
	close(b.StopChan)
	if b.Conn != nil {
		b.Conn.Close()
	}
}
