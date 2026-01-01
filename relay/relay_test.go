package main

import (
	"fmt"
	"log"
	"net"
	"os"
	"sync"
	"testing"
	"time"

	"github.com/tri/orb/node"
)

// TestRelayStartStop tests basic relay lifecycle
func TestRelayStartStop(t *testing.T) {
	relay := NewRelay()

	// Start relay
	addr := "127.0.0.1:0"
	if err := relay.Listen(addr); err != nil {
		t.Fatalf("Failed to listen: %v", err)
	}

	go relay.Serve()

	// Give it a moment to start
	time.Sleep(100 * time.Millisecond)

	// Shutdown relay
	relay.Shutdown()
}

// TestNodeRegister tests node registration
func TestNodeRegister(t *testing.T) {
	relay := NewRelay()

	addr := "127.0.0.1:0"
	if err := relay.Listen(addr); err != nil {
		t.Fatalf("Failed to listen: %v", err)
	}
	defer relay.Shutdown()

	go relay.Serve()

	relayAddr := relay.Addr().String()

	// Connect as a node
	conn, err := net.Dial("tcp", relayAddr)
	if err != nil {
		t.Fatalf("Failed to connect to relay: %v", err)
	}
	defer conn.Close()

	nodeConn := node.NewConn(conn)

	// Send REGISTER
	regMsg := node.NewRegisterMsg("msg-1", "node-1")
	if err := nodeConn.WriteMessage(regMsg); err != nil {
		t.Fatalf("Failed to send register: %v", err)
	}

	// Read ACK
	msg, err := nodeConn.ReadMessage()
	if err != nil {
		t.Fatalf("Failed to read ack: %v", err)
	}

	if !msg.IsControl() || msg.ControlType() != node.MsgTypeAck {
		t.Fatalf("Expected ACK, got: %+v", msg)
	}

	// Verify node is registered
	time.Sleep(50 * time.Millisecond)
	relay.nodesMu.RLock()
	_, exists := relay.nodes["node-1"]
	relay.nodesMu.RUnlock()

	if !exists {
		t.Fatal("Node should be registered")
	}
}

// TestServiceAnnounce tests service announcement
func TestServiceAnnounce(t *testing.T) {
	relay := NewRelay()

	addr := "127.0.0.1:0"
	if err := relay.Listen(addr); err != nil {
		t.Fatalf("Failed to listen: %v", err)
	}
	defer relay.Shutdown()

	go relay.Serve()

	relayAddr := relay.Addr().String()

	// Connect and register
	conn, err := net.Dial("tcp", relayAddr)
	if err != nil {
		t.Fatalf("Failed to connect: %v", err)
	}
	defer conn.Close()

	nodeConn := node.NewConn(conn)

	// Register
	regMsg := node.NewRegisterMsg("msg-1", "node-1")
	if err := nodeConn.WriteMessage(regMsg); err != nil {
		t.Fatalf("Failed to send register: %v", err)
	}

	// Read ACK
	if _, err := nodeConn.ReadMessage(); err != nil {
		t.Fatalf("Failed to read ack: %v", err)
	}

	// Announce services
	services := []node.Service{
		{ID: "svc-1", Type: "rtsp", Addr: "192.168.1.100", Port: 554},
		{ID: "svc-2", Type: "http", Addr: "192.168.1.101", Port: 80},
	}

	announceMsg := node.NewAnnounceMsg("msg-2", services)
	if err := nodeConn.WriteMessage(announceMsg); err != nil {
		t.Fatalf("Failed to send announce: %v", err)
	}

	// Read ACK
	if _, err := nodeConn.ReadMessage(); err != nil {
		t.Fatalf("Failed to read announce ack: %v", err)
	}

	// Verify services are registered
	time.Sleep(50 * time.Millisecond)
	registeredServices := relay.Services().List()

	if len(registeredServices) != 2 {
		t.Fatalf("Expected 2 services, got %d", len(registeredServices))
	}
}

// TestOpenBridge tests bridge opening
func TestOpenBridge(t *testing.T) {
	relay := NewRelay()

	addr := "127.0.0.1:0"
	if err := relay.Listen(addr); err != nil {
		t.Fatalf("Failed to listen: %v", err)
	}
	defer relay.Shutdown()

	go relay.Serve()

	relayAddr := relay.Addr().String()

	// Start a mock service
	mockAddr := startMockService(t)

	// Connect and register node
	conn, err := net.Dial("tcp", relayAddr)
	if err != nil {
		t.Fatalf("Failed to connect: %v", err)
	}
	defer conn.Close()

	nodeConn := node.NewConn(conn)

	// Register
	regMsg := node.NewRegisterMsg("msg-1", "node-1")
	if err := nodeConn.WriteMessage(regMsg); err != nil {
		t.Fatalf("Failed to send register: %v", err)
	}
	nodeConn.ReadMessage() // ACK

	// Announce service
	services := []node.Service{
		{ID: "test-svc", Type: "tcp", Addr: mockAddr.IP.String(), Port: mockAddr.Port},
	}
	announceMsg := node.NewAnnounceMsg("msg-2", services)
	if err := nodeConn.WriteMessage(announceMsg); err != nil {
		t.Fatalf("Failed to send announce: %v", err)
	}
	nodeConn.ReadMessage() // ACK

	time.Sleep(50 * time.Millisecond)

	// Request bridge opening
	relay.nodesMu.RLock()
	nc, exists := relay.nodes["node-1"]
	relay.nodesMu.RUnlock()

	if !exists {
		t.Fatal("Node not found")
	}

	bridgeID, err := nc.OpenBridge(services[0])
	if err != nil {
		t.Fatalf("Failed to open bridge: %v", err)
	}

	// Verify bridge is open
	if bridgeID == "" {
		t.Fatal("Bridge ID should not be empty")
	}

	// Cleanup
	nc.CloseBridge(bridgeID)
}

// TestEndToEndWithRealService tests the full flow with RTSP service from config.json
func TestEndToEndWithRealService(t *testing.T) {
	// Load real services from config.json
	allowList, err := node.LoadAllowList("config.json")
	if err != nil {
		t.Fatalf("Failed to load config.json: %v", err)
	}

	// Find RTSP service
	var rtspService *node.Service
	for i, svc := range allowList.Services {
		if svc.Type == "rtsp" {
			rtspService = &allowList.Services[i]
			break
		}
	}

	if rtspService == nil {
		t.Skip("No RTSP service found in config.json")
	}

	log.Printf("[Test] Using RTSP service: %s at %s:%d%s", rtspService.ID, rtspService.Addr, rtspService.Port, rtspService.Path)

	// Start relay
	relay := NewRelay()
	relayAddr := "127.0.0.1:0"
	if err := relay.Listen(relayAddr); err != nil {
		t.Fatalf("Failed to listen: %v", err)
	}
	defer relay.Shutdown()

	go relay.Serve()

	actualRelayAddr := relay.Addr().String()
	log.Printf("[Test] Relay started at %s", actualRelayAddr)

	// Start node client
	nodeClient := node.NewNodeClient(actualRelayAddr, "test-node", allowList.Services)

	var wg sync.WaitGroup
	wg.Add(1)
	go func() {
		defer wg.Done()
		if err := nodeClient.Run(); err != nil {
			log.Printf("[Test] Node client error: %v", err)
		}
	}()

	// Give node time to register and announce
	time.Sleep(500 * time.Millisecond)

	// Verify services are registered in relay
	services := relay.Services().List()
	log.Printf("[Test] Relay has %d services registered", len(services))

	if len(services) == 0 {
		t.Fatal("No services registered in relay")
	}

	// Find the node connection
	relay.nodesMu.RLock()
	nc, exists := relay.nodes["test-node"]
	relay.nodesMu.RUnlock()

	if !exists {
		t.Fatal("Node not registered")
	}

	// Open bridge to RTSP service
	log.Printf("[Test] Opening bridge to RTSP service: %s", rtspService.ID)
	bridgeID, err := nc.OpenBridge(*rtspService)
	if err != nil {
		t.Fatalf("Failed to open bridge: %v", err)
	}

	log.Printf("[Test] Bridge opened: %s", bridgeID)

	// Send RTSP DESCRIBE request to trigger a response
	rtspDescribe := fmt.Sprintf("DESCRIBE rtsp://%s:%d%s RTSP/1.0\r\nCSeq: 1\r\nAccept: application/sdp\r\n\r\n",
		rtspService.Addr, rtspService.Port, rtspService.Path)

	if err := nc.SendData(bridgeID, []byte(rtspDescribe)); err != nil {
		t.Fatalf("Failed to send RTSP DESCRIBE: %v", err)
	}

	log.Printf("[Test] Sent RTSP DESCRIBE request")

	// Wait for response
	time.Sleep(2 * time.Second)

	// Check received data
	receivedData := nodeClient.GetReceivedData(bridgeID)
	log.Printf("[Test] Received %d bytes from RTSP service", len(receivedData))

	if len(receivedData) == 0 {
		t.Fatal("No data received from RTSP service")
	}

	// Verify it looks like RTSP response (should start with "RTSP/1.0")
	responseStr := string(receivedData)
	if len(responseStr) > 8 && responseStr[:8] != "RTSP/1.0" {
		log.Printf("[Test] Response preview: %s", responseStr[:min(200, len(responseStr))])
		t.Logf("Warning: Response doesn't start with RTSP/1.0, might not be RTSP protocol")
	} else {
		log.Printf("[Test] Valid RTSP response received")
		log.Printf("[Test] Response preview: %s", responseStr[:min(500, len(responseStr))])
	}

	// Close bridge
	nc.CloseBridge(bridgeID)
	log.Printf("[Test] Bridge closed")

	// Cleanup
	nodeClient.Close()
	wg.Wait()

	log.Printf("[Test] End-to-end test completed successfully")
}

// startMockService starts a simple TCP echo server for testing
func startMockService(t *testing.T) *net.TCPAddr {
	listener, err := net.Listen("tcp", "127.0.0.1:0")
	if err != nil {
		t.Fatalf("Failed to start mock service: %v", err)
	}

	go func() {
		for {
			conn, err := listener.Accept()
			if err != nil {
				return
			}
			go func(c net.Conn) {
				defer c.Close()
				buf := make([]byte, 1024)
				for {
					n, err := c.Read(buf)
					if err != nil {
						return
					}
					c.Write(buf[:n]) // Echo back
				}
			}(conn)
		}
	}()

	return listener.Addr().(*net.TCPAddr)
}

func init() {
	// Enable logging for tests
	log.SetOutput(os.Stdout)
	log.SetFlags(log.Ltime | log.Lmicroseconds)
}
