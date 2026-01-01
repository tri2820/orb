package main

import (
	"fmt"
	"log"
	"net"
	"sync"

	"github.com/tri/orb/node"
)

// Server manages node connections and the service registry
type Server struct {
	listener net.Listener
	nodes    map[string]*NodeConn // node_id -> connection
	nodesMu  sync.RWMutex
	services *ServiceRegistry

	shutdown chan struct{}
	wg       sync.WaitGroup
}

// NewServer creates a new relay server
func NewServer() *Server {
	return &Server{
		nodes:    make(map[string]*NodeConn),
		services: NewServiceRegistry(),
		shutdown: make(chan struct{}),
	}
}

// Listen starts listening on the given address
func (s *Server) Listen(addr string) error {
	ln, err := net.Listen("tcp", addr)
	if err != nil {
		return fmt.Errorf("listen: %w", err)
	}
	s.listener = ln
	log.Printf("[Server] Listening on %s", addr)
	return nil
}

// Serve accepts and handles node connections
func (s *Server) Serve() error {
	for {
		conn, err := s.listener.Accept()
		if err != nil {
			select {
			case <-s.shutdown:
				return nil // Clean shutdown
			default:
				log.Printf("[Server] Accept error: %v", err)
				continue
			}
		}

		s.wg.Add(1)
		go func() {
			defer s.wg.Done()
			s.handleConnection(conn)
		}()
	}
}

// handleConnection handles a single node connection
func (s *Server) handleConnection(conn net.Conn) {
	framedConn := node.NewConn(conn)
	nodeConn := &NodeConn{
		conn:     framedConn,
		server:   s,
		bridges:  make(map[string]*Bridge),
		shutdown: make(chan struct{}),
	}

	log.Printf("[Server] New connection from %s", conn.RemoteAddr())

	// Handle messages until connection closes
	if err := nodeConn.Run(); err != nil {
		log.Printf("[Server] Connection error: %v", err)
	}

	// Cleanup on disconnect
	s.removeNode(nodeConn)
	log.Printf("[Server] Connection closed from %s", conn.RemoteAddr())
}

// registerNode adds a node to the registry
func (s *Server) registerNode(nodeID string, nc *NodeConn) {
	s.nodesMu.Lock()
	defer s.nodesMu.Unlock()

	// Remove old connection if exists
	if old, exists := s.nodes[nodeID]; exists {
		old.Close()
	}

	s.nodes[nodeID] = nc
	nc.nodeID = nodeID
	log.Printf("[Server] Node registered: %s", nodeID)
}

// removeNode removes a node and its services
func (s *Server) removeNode(nc *NodeConn) {
	if nc.nodeID == "" {
		return
	}

	s.nodesMu.Lock()
	delete(s.nodes, nc.nodeID)
	s.nodesMu.Unlock()

	// Remove all services from this node
	s.services.RemoveByNode(nc.nodeID)
	log.Printf("[Server] Node removed: %s", nc.nodeID)
}

// GetNode returns a node connection by ID
func (s *Server) GetNode(nodeID string) *NodeConn {
	s.nodesMu.RLock()
	defer s.nodesMu.RUnlock()
	return s.nodes[nodeID]
}

// ListNodes returns a list of connected node IDs
func (s *Server) ListNodes() []string {
	s.nodesMu.RLock()
	defer s.nodesMu.RUnlock()

	result := make([]string, 0, len(s.nodes))
	for id := range s.nodes {
		result = append(result, id)
	}
	return result
}

// Services returns the service registry
func (s *Server) Services() *ServiceRegistry {
	return s.services
}

// Shutdown gracefully shuts down the server
func (s *Server) Shutdown() {
	close(s.shutdown)
	if s.listener != nil {
		s.listener.Close()
	}

	// Close all node connections
	s.nodesMu.Lock()
	for _, nc := range s.nodes {
		nc.Close()
	}
	s.nodesMu.Unlock()

	s.wg.Wait()
	log.Printf("[Server] Shutdown complete")
}

// Addr returns the listener address
func (s *Server) Addr() net.Addr {
	if s.listener == nil {
		return nil
	}
	return s.listener.Addr()
}
