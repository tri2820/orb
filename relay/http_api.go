package main

import (
	"encoding/json"
	"fmt"
	"log"
	"net/http"
)

func startHTTPAPI(server *Server, addr string) {
	mux := http.NewServeMux()

	// List available services
	mux.HandleFunc("/services", func(w http.ResponseWriter, r *http.Request) {
		services := server.Services().List()
		w.Header().Set("Content-Type", "application/json")
		json.NewEncoder(w).Encode(services)
	})

	// List nodes
	mux.HandleFunc("/nodes", func(w http.ResponseWriter, r *http.Request) {
		nodes := server.ListNodes()
		w.Header().Set("Content-Type", "application/json")
		json.NewEncoder(w).Encode(nodes)
	})

	// Open bridge to a service
	mux.HandleFunc("/bridge/open", func(w http.ResponseWriter, r *http.Request) {
		if r.Method != http.MethodPost {
			http.Error(w, "Method not allowed", http.StatusMethodNotAllowed)
			return
		}

		var req struct {
			ServiceID string `json:"service_id"`
		}
		if err := json.NewDecoder(r.Body).Decode(&req); err != nil {
			http.Error(w, "Invalid request", http.StatusBadRequest)
			return
		}

		// Find service
		service := server.Services().GetByID(req.ServiceID)
		if service == nil {
			http.Error(w, fmt.Sprintf("Service not found: %s", req.ServiceID), http.StatusNotFound)
			return
		}

		// Find node
		nodeID := server.Services().GetNodeID(req.ServiceID)
		if nodeID == "" {
			http.Error(w, fmt.Sprintf("No node for service: %s", req.ServiceID), http.StatusNotFound)
			return
		}

		nodeConn := server.GetNode(nodeID)
		if nodeConn == nil {
			http.Error(w, fmt.Sprintf("Node not connected: %s", nodeID), http.StatusNotFound)
			return
		}

		// Open bridge
		bridgeID, err := nodeConn.OpenBridge(*service)
		if err != nil {
			http.Error(w, fmt.Sprintf("Failed to open bridge: %v", err), http.StatusInternalServerError)
			return
		}

		log.Printf("[HTTP] Opened bridge %s to service %s", bridgeID, req.ServiceID)

		w.Header().Set("Content-Type", "application/json")
		json.NewEncoder(w).Encode(map[string]string{
			"bridge_id": bridgeID,
			"service_id": req.ServiceID,
			"node_id": nodeID,
		})
	})

	// Close bridge
	mux.HandleFunc("/bridge/close", func(w http.ResponseWriter, r *http.Request) {
		if r.Method != http.MethodPost {
			http.Error(w, "Method not allowed", http.StatusMethodNotAllowed)
			return
		}

		var req struct {
			BridgeID string `json:"bridge_id"`
			NodeID   string `json:"node_id"`
		}
		if err := json.NewDecoder(r.Body).Decode(&req); err != nil {
			http.Error(w, "Invalid request", http.StatusBadRequest)
			return
		}

		nodeConn := server.GetNode(req.NodeID)
		if nodeConn == nil {
			http.Error(w, fmt.Sprintf("Node not connected: %s", req.NodeID), http.StatusNotFound)
			return
		}

		if err := nodeConn.CloseBridge(req.BridgeID); err != nil {
			http.Error(w, fmt.Sprintf("Failed to close bridge: %v", err), http.StatusInternalServerError)
			return
		}

		log.Printf("[HTTP] Closed bridge %s", req.BridgeID)

		w.Header().Set("Content-Type", "application/json")
		json.NewEncoder(w).Encode(map[string]string{"status": "ok"})
	})

	log.Printf("[HTTP] API listening on %s", addr)
	if err := http.ListenAndServe(addr, mux); err != nil {
		log.Printf("[HTTP] Error: %v", err)
	}
}
