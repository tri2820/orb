package main

import (
	"net"
	"testing"
	"time"

	"github.com/tri/orb/node"
)

// loadConfig loads config.json
func loadConfig(t *testing.T) *Config {
	config, err := LoadConfig("config.json")
	if err != nil {
		t.Fatalf("Failed to load config.json: %v", err)
	}
	return config
}

// connectNode creates a node connection to the server
func connectNode(t *testing.T, addr string) *node.Conn {
	conn, err := net.Dial("tcp", addr)
	if err != nil {
		t.Fatalf("Failed to connect: %v", err)
	}
	return node.NewConn(conn)
}

func TestServerStartStop(t *testing.T) {
	server := NewServer()
	if err := server.Listen("127.0.0.1:0"); err != nil {
		t.Fatalf("Listen failed: %v", err)
	}

	go server.Serve()

	// Give it a moment to start
	time.Sleep(10 * time.Millisecond)

	server.Shutdown()
}

func TestNodeRegister(t *testing.T) {
	server := NewServer()
	if err := server.Listen("127.0.0.1:0"); err != nil {
		t.Fatalf("Listen failed: %v", err)
	}
	defer server.Shutdown()

	go server.Serve()
	time.Sleep(10 * time.Millisecond)

	// Connect as node
	nodeConn := connectNode(t, server.Addr().String())
	defer nodeConn.Close()

	// Send REGISTER
	regMsg := node.NewRegisterMsg("msg-1", "test-node-1")
	if err := nodeConn.WriteMessage(regMsg); err != nil {
		t.Fatalf("WriteMessage failed: %v", err)
	}

	// Expect ACK
	ack, err := nodeConn.ReadMessage()
	if err != nil {
		t.Fatalf("ReadMessage failed: %v", err)
	}

	if ack.ControlType() != node.MsgTypeAck {
		t.Errorf("Expected ACK, got %s", ack.ControlType())
	}
	if ack.Control.AckMsgID != "msg-1" {
		t.Errorf("AckMsgID = %q, want %q", ack.Control.AckMsgID, "msg-1")
	}

	// Verify node is registered
	time.Sleep(10 * time.Millisecond) // Let server process
	if server.GetNode("test-node-1") == nil {
		t.Error("Node not registered")
	}
}

func TestNodeAnnounce(t *testing.T) {
	server := NewServer()
	if err := server.Listen("127.0.0.1:0"); err != nil {
		t.Fatalf("Listen failed: %v", err)
	}
	defer server.Shutdown()

	go server.Serve()
	time.Sleep(10 * time.Millisecond)

	// Connect and register
	nodeConn := connectNode(t, server.Addr().String())
	defer nodeConn.Close()

	regMsg := node.NewRegisterMsg("msg-1", "test-node-1")
	nodeConn.WriteMessage(regMsg)
	nodeConn.ReadMessage() // ACK

	// Send ANNOUNCE
	services := []node.Service{
		{ID: "cam-1", Addr: "192.168.1.100", Port: 554, Type: "rtsp"},
		{ID: "cam-2", Addr: "192.168.1.101", Port: 8080, Type: "mjpeg"},
	}
	announceMsg := node.NewAnnounceMsg("msg-2", services)
	if err := nodeConn.WriteMessage(announceMsg); err != nil {
		t.Fatalf("WriteMessage failed: %v", err)
	}

	// Expect ACK
	ack, err := nodeConn.ReadMessage()
	if err != nil {
		t.Fatalf("ReadMessage failed: %v", err)
	}

	if ack.Control.AckMsgID != "msg-2" {
		t.Errorf("AckMsgID = %q, want %q", ack.Control.AckMsgID, "msg-2")
	}

	// Verify services are registered
	time.Sleep(10 * time.Millisecond)
	if server.Services().Count() != 2 {
		t.Errorf("Service count = %d, want 2", server.Services().Count())
	}

	svc := server.Services().Get("cam-1")
	if svc == nil {
		t.Fatal("Service cam-1 not found")
	}
	if svc.Service.Addr != "192.168.1.100" {
		t.Errorf("Addr = %q, want %q", svc.Service.Addr, "192.168.1.100")
	}
	if svc.NodeID != "test-node-1" {
		t.Errorf("NodeID = %q, want %q", svc.NodeID, "test-node-1")
	}
}

func TestMultipleNodes(t *testing.T) {
	server := NewServer()
	if err := server.Listen("127.0.0.1:0"); err != nil {
		t.Fatalf("Listen failed: %v", err)
	}
	defer server.Shutdown()

	go server.Serve()
	time.Sleep(10 * time.Millisecond)

	// Connect two nodes
	node1 := connectNode(t, server.Addr().String())
	defer node1.Close()
	node2 := connectNode(t, server.Addr().String())
	defer node2.Close()

	// Register both
	node1.WriteMessage(node.NewRegisterMsg("m1", "node-1"))
	node1.ReadMessage()
	node2.WriteMessage(node.NewRegisterMsg("m2", "node-2"))
	node2.ReadMessage()

	// Announce different services
	node1.WriteMessage(node.NewAnnounceMsg("m3", []node.Service{
		{ID: "svc-a", Addr: "10.0.0.1", Port: 80},
	}))
	node1.ReadMessage()

	node2.WriteMessage(node.NewAnnounceMsg("m4", []node.Service{
		{ID: "svc-b", Addr: "10.0.0.2", Port: 80},
		{ID: "svc-c", Addr: "10.0.0.3", Port: 80},
	}))
	node2.ReadMessage()

	time.Sleep(10 * time.Millisecond)

	// Verify
	if server.Services().Count() != 3 {
		t.Errorf("Service count = %d, want 3", server.Services().Count())
	}

	svcA := server.Services().Get("svc-a")
	if svcA == nil || svcA.NodeID != "node-1" {
		t.Error("svc-a not correctly registered")
	}

	svcB := server.Services().Get("svc-b")
	if svcB == nil || svcB.NodeID != "node-2" {
		t.Error("svc-b not correctly registered")
	}
}

func TestNodeDisconnect(t *testing.T) {
	server := NewServer()
	if err := server.Listen("127.0.0.1:0"); err != nil {
		t.Fatalf("Listen failed: %v", err)
	}
	defer server.Shutdown()

	go server.Serve()
	time.Sleep(10 * time.Millisecond)

	// Connect and register
	nodeConn := connectNode(t, server.Addr().String())

	nodeConn.WriteMessage(node.NewRegisterMsg("m1", "test-node"))
	nodeConn.ReadMessage()

	nodeConn.WriteMessage(node.NewAnnounceMsg("m2", []node.Service{
		{ID: "svc-1", Addr: "192.168.1.1", Port: 554},
	}))
	nodeConn.ReadMessage()

	time.Sleep(10 * time.Millisecond)
	if server.Services().Count() != 1 {
		t.Fatalf("Expected 1 service, got %d", server.Services().Count())
	}

	// Disconnect
	nodeConn.Close()
	time.Sleep(50 * time.Millisecond) // Let server detect disconnect

	// Services should be removed
	if server.Services().Count() != 0 {
		t.Errorf("Expected 0 services after disconnect, got %d", server.Services().Count())
	}

	if server.GetNode("test-node") != nil {
		t.Error("Node should be removed after disconnect")
	}
}

func TestServiceRegistry(t *testing.T) {
	reg := NewServiceRegistry()

	// Add services
	reg.Register(node.Service{ID: "s1", Addr: "1.1.1.1", Port: 80}, "node-a")
	reg.Register(node.Service{ID: "s2", Addr: "2.2.2.2", Port: 80}, "node-a")
	reg.Register(node.Service{ID: "s3", Addr: "3.3.3.3", Port: 80}, "node-b")

	if reg.Count() != 3 {
		t.Errorf("Count = %d, want 3", reg.Count())
	}

	// Get
	s1 := reg.Get("s1")
	if s1 == nil || s1.Service.Addr != "1.1.1.1" {
		t.Error("Get s1 failed")
	}

	// List by node
	nodeAServices := reg.ListByNode("node-a")
	if len(nodeAServices) != 2 {
		t.Errorf("ListByNode(node-a) = %d, want 2", len(nodeAServices))
	}

	// Remove by node
	reg.RemoveByNode("node-a")
	if reg.Count() != 1 {
		t.Errorf("Count after RemoveByNode = %d, want 1", reg.Count())
	}

	if reg.Get("s1") != nil {
		t.Error("s1 should be removed")
	}
	if reg.Get("s3") == nil {
		t.Error("s3 should still exist")
	}
}

func TestNodeAnnounceFromConfig(t *testing.T) {
	config := loadConfig(t)

	server := NewServer()
	if err := server.Listen("127.0.0.1:0"); err != nil {
		t.Fatalf("Listen failed: %v", err)
	}
	defer server.Shutdown()

	go server.Serve()
	time.Sleep(10 * time.Millisecond)

	// Connect and register
	nodeConn := connectNode(t, server.Addr().String())
	defer nodeConn.Close()

	regMsg := node.NewRegisterMsg("msg-1", "config-node")
	nodeConn.WriteMessage(regMsg)
	nodeConn.ReadMessage() // ACK

	// Announce services from config.json
	announceMsg := node.NewAnnounceMsg("msg-2", config.Services)
	if err := nodeConn.WriteMessage(announceMsg); err != nil {
		t.Fatalf("WriteMessage failed: %v", err)
	}

	// Expect ACK
	ack, err := nodeConn.ReadMessage()
	if err != nil {
		t.Fatalf("ReadMessage failed: %v", err)
	}
	if ack.Control.AckMsgID != "msg-2" {
		t.Errorf("AckMsgID = %q, want %q", ack.Control.AckMsgID, "msg-2")
	}

	// Verify all services from config are registered
	time.Sleep(10 * time.Millisecond)
	if server.Services().Count() != len(config.Services) {
		t.Errorf("Service count = %d, want %d", server.Services().Count(), len(config.Services))
	}

	// Verify each service
	for _, svc := range config.Services {
		registered := server.Services().Get(svc.ID)
		if registered == nil {
			t.Errorf("Service %q not found", svc.ID)
			continue
		}
		if registered.Service.Addr != svc.Addr {
			t.Errorf("Service %q Addr = %q, want %q", svc.ID, registered.Service.Addr, svc.Addr)
		}
		if registered.Service.Port != svc.Port {
			t.Errorf("Service %q Port = %d, want %d", svc.ID, registered.Service.Port, svc.Port)
		}
		if registered.Service.Type != svc.Type {
			t.Errorf("Service %q Type = %q, want %q", svc.ID, registered.Service.Type, svc.Type)
		}
		if registered.Service.Path != svc.Path {
			t.Errorf("Service %q Path = %q, want %q", svc.ID, registered.Service.Path, svc.Path)
		}
		if registered.NodeID != "config-node" {
			t.Errorf("Service %q NodeID = %q, want %q", svc.ID, registered.NodeID, "config-node")
		}
		t.Logf("✓ Service: %s | Type: %s | Addr: %s:%d", svc.ID, svc.Type, svc.Addr, svc.Port)
	}
}

func BenchmarkRegisterAnnounce(b *testing.B) {
	server := NewServer()
	server.Listen("127.0.0.1:0")
	defer server.Shutdown()

	go server.Serve()
	time.Sleep(10 * time.Millisecond)

	conn, _ := net.Dial("tcp", server.Addr().String())
	nodeConn := node.NewConn(conn)
	defer nodeConn.Close()

	b.ResetTimer()
	for i := 0; i < b.N; i++ {
		nodeConn.WriteMessage(node.NewRegisterMsg("m", "node"))
		nodeConn.ReadMessage()
	}
}
