package main

import (
	"fmt"
	"log"
	"sync"

	"github.com/google/uuid"
	"github.com/tri/orb/node"
)

// Bridge represents an active data bridge
type Bridge struct {
	ID        string
	Service   node.Service
	ByteCount int64
	MsgCount  int64
}

// NodeConn handles a single node connection
type NodeConn struct {
	conn     *node.Conn
	relay    *Relay
	nodeID   string
	bridges  map[string]*Bridge
	bridgeMu sync.RWMutex
	shutdown chan struct{}
	closed   bool
	closeMu  sync.Mutex
}

// Run handles incoming messages from the node
func (nc *NodeConn) Run() error {
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
			log.Printf("[NodeConn] Error handling message: %v", err)
		}
	}
}

// handleMessage dispatches a message to the appropriate handler
func (nc *NodeConn) handleMessage(msg *node.Message) error {
	if msg.IsControl() {
		return nc.handleControl(msg)
	}
	if msg.IsData() {
		return nc.handleData(msg)
	}
	return fmt.Errorf("unknown message type")
}

// handleControl handles control messages
func (nc *NodeConn) handleControl(msg *node.Message) error {
	ctrl := msg.Control

	switch ctrl.Type {
	case node.MsgTypeRegister:
		return nc.handleRegister(msg.MsgID, ctrl.NodeID)

	case node.MsgTypeAnnounce:
		return nc.handleAnnounce(msg.MsgID, ctrl.Services)

	case node.MsgTypeAck:
		// Node acknowledging our message - log and ignore for now
		log.Printf("[NodeConn %s] Received ACK for %s", nc.nodeID, ctrl.AckMsgID)
		return nil

	default:
		return fmt.Errorf("unknown control type: %s", ctrl.Type)
	}
}

// handleRegister handles REGISTER messages
func (nc *NodeConn) handleRegister(msgID, nodeID string) error {
	if nodeID == "" {
		return fmt.Errorf("empty node_id")
	}

	nc.relay.registerNode(nodeID, nc)

	// Send ACK
	return nc.sendAck(msgID)
}

// handleAnnounce handles ANNOUNCE messages
func (nc *NodeConn) handleAnnounce(msgID string, services []node.Service) error {
	if nc.nodeID == "" {
		return fmt.Errorf("node not registered")
	}

	// Register services
	for _, svc := range services {
		nc.relay.services.Register(svc, nc.nodeID)
		log.Printf("[NodeConn %s] Service announced: %s (%s:%d)",
			nc.nodeID, svc.ID, svc.Addr, svc.Port)
	}

	// Send ACK
	return nc.sendAck(msgID)
}

// handleData handles DATA messages
func (nc *NodeConn) handleData(msg *node.Message) error {
	data := msg.Data

	nc.bridgeMu.Lock()
	bridge, exists := nc.bridges[data.BridgeID]
	if !exists {
		nc.bridgeMu.Unlock()
		return fmt.Errorf("unknown bridge: %s", data.BridgeID)
	}

	// Update statistics
	bridge.MsgCount++
	bridge.ByteCount += int64(len(data.Payload))
	nc.bridgeMu.Unlock()

	// Log first 32 bytes in hex for debugging
	hexLen := 32
	if len(data.Payload) < hexLen {
		hexLen = len(data.Payload)
	}
	hexStr := ""
	for i := 0; i < hexLen; i++ {
		hexStr += fmt.Sprintf("%02x ", data.Payload[i])
	}

	log.Printf("[NodeConn %s] Bridge %s (%s): msg #%d, %d bytes, payload[0:%d]=%s",
		nc.nodeID, bridge.ID, bridge.Service.ID, bridge.MsgCount, len(data.Payload), hexLen, hexStr)

	// TODO: Forward data to client (go2rtc integration or other consumer)
	// For now, we're just logging to verify data flow

	return nil
}

// sendAck sends an ACK message
func (nc *NodeConn) sendAck(ackMsgID string) error {
	ack := node.NewAckMsg(uuid.New().String(), ackMsgID)
	return nc.conn.WriteMessage(ack)
}

// OpenBridge requests the node to open a bridge to a service
func (nc *NodeConn) OpenBridge(service node.Service) (string, error) {
	bridgeID := uuid.New().String()
	msgID := uuid.New().String()

	msg := node.NewOpenBridgeMsg(msgID, bridgeID, &service)
	if err := nc.conn.WriteMessage(msg); err != nil {
		return "", fmt.Errorf("send open_bridge: %w", err)
	}

	// Store bridge
	nc.bridgeMu.Lock()
	nc.bridges[bridgeID] = &Bridge{
		ID:      bridgeID,
		Service: service,
	}
	nc.bridgeMu.Unlock()

	log.Printf("[NodeConn %s] Opened bridge %s to %s:%d",
		nc.nodeID, bridgeID, service.Addr, service.Port)

	return bridgeID, nil
}

// CloseBridge requests the node to close a bridge
func (nc *NodeConn) CloseBridge(bridgeID string) error {
	nc.bridgeMu.Lock()
	delete(nc.bridges, bridgeID)
	nc.bridgeMu.Unlock()

	msgID := uuid.New().String()
	msg := node.NewCloseBridgeMsg(msgID, bridgeID)
	if err := nc.conn.WriteMessage(msg); err != nil {
		return fmt.Errorf("send close_bridge: %w", err)
	}

	log.Printf("[NodeConn %s] Closed bridge %s", nc.nodeID, bridgeID)
	return nil
}

// SendData sends data over a bridge
func (nc *NodeConn) SendData(bridgeID string, payload []byte) error {
	msgID := uuid.New().String()
	msg := node.NewDataMsg(msgID, bridgeID, payload)
	return nc.conn.WriteMessage(msg)
}

// Close closes the node connection
func (nc *NodeConn) Close() {
	nc.closeMu.Lock()
	defer nc.closeMu.Unlock()

	if nc.closed {
		return
	}
	nc.closed = true

	close(nc.shutdown)
	nc.conn.Close()
}

// NodeID returns the node's ID
func (nc *NodeConn) NodeID() string {
	return nc.nodeID
}
