package main

import (
	"fmt"
	"log"
	"net"
	"sync"

	"github.com/tri/orb/node"
)

// Relay manages node connections and the service registry
type Relay struct {
	listener net.Listener
	nodes    map[string]*NodeConn // node_id -> connection
	nodesMu  sync.RWMutex
	services *ServiceRegistry

	shutdown chan struct{}
	wg       sync.WaitGroup
}

// NewRelay creates a new relay server
func NewRelay() *Relay {
	return &Relay{
		nodes:    make(map[string]*NodeConn),
		services: NewServiceRegistry(),
		shutdown: make(chan struct{}),
	}
}

// Listen starts listening on the given address
func (r *Relay) Listen(addr string) error {
	ln, err := net.Listen("tcp", addr)
	if err != nil {
		return fmt.Errorf("listen: %w", err)
	}
	r.listener = ln
	log.Printf("[Relay] Listening on %s", addr)
	return nil
}

// Serve accepts and handles node connections
func (r *Relay) Serve() error {
	for {
		conn, err := r.listener.Accept()
		if err != nil {
			select {
			case <-r.shutdown:
				return nil // Clean shutdown
			default:
				log.Printf("[Relay] Accept error: %v", err)
				continue
			}
		}

		r.wg.Add(1)
		go func() {
			defer r.wg.Done()
			r.handleConnection(conn)
		}()
	}
}

// handleConnection handles a single node connection
func (r *Relay) handleConnection(conn net.Conn) {
	framedConn := node.NewConn(conn)
	nodeConn := &NodeConn{
		conn:     framedConn,
		relay:    r,
		bridges:  make(map[string]*Bridge),
		shutdown: make(chan struct{}),
	}

	log.Printf("[Relay] New connection from %s", conn.RemoteAddr())

	// Handle messages until connection closes
	if err := nodeConn.Run(); err != nil {
		log.Printf("[Relay] Connection error: %v", err)
	}

	// Cleanup on disconnect
	r.removeNode(nodeConn)
	log.Printf("[Relay] Connection closed from %s", conn.RemoteAddr())
}

// registerNode adds a node to the registry
func (r *Relay) registerNode(nodeID string, nc *NodeConn) {
	r.nodesMu.Lock()
	defer r.nodesMu.Unlock()

	// Remove old connection if exists
	if old, exists := r.nodes[nodeID]; exists {
		old.Close()
	}

	r.nodes[nodeID] = nc
	nc.nodeID = nodeID
	log.Printf("[Relay] Node registered: %s", nodeID)
}

// removeNode removes a node and its services
func (r *Relay) removeNode(nc *NodeConn) {
	if nc.nodeID == "" {
		return
	}

	r.nodesMu.Lock()
	delete(r.nodes, nc.nodeID)
	r.nodesMu.Unlock()

	// Remove all services from this node
	r.services.RemoveByNode(nc.nodeID)
	log.Printf("[Relay] Node removed: %s", nc.nodeID)
}

// GetNode returns a node connection by ID
func (r *Relay) GetNode(nodeID string) *NodeConn {
	r.nodesMu.RLock()
	defer r.nodesMu.RUnlock()
	return r.nodes[nodeID]
}

// ListNodes returns a list of connected node IDs
func (r *Relay) ListNodes() []string {
	r.nodesMu.RLock()
	defer r.nodesMu.RUnlock()

	result := make([]string, 0, len(r.nodes))
	for id := range r.nodes {
		result = append(result, id)
	}
	return result
}

// Services returns the service registry
func (r *Relay) Services() *ServiceRegistry {
	return r.services
}

// Shutdown gracefully shuts down the relay
func (r *Relay) Shutdown() {
	close(r.shutdown)
	if r.listener != nil {
		r.listener.Close()
	}

	// Close all node connections
	r.nodesMu.Lock()
	for _, nc := range r.nodes {
		nc.Close()
	}
	r.nodesMu.Unlock()

	r.wg.Wait()
	log.Printf("[Relay] Shutdown complete")
}

// Addr returns the listener address
func (r *Relay) Addr() net.Addr {
	if r.listener == nil {
		return nil
	}
	return r.listener.Addr()
}
