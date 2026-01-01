package main

import (
	"encoding/json"
	"net/http"
)

func startHTTPAPI(relay *Relay, addr string) {
	mux := http.NewServeMux()

	// List available services
	mux.HandleFunc("/services", func(w http.ResponseWriter, r *http.Request) {
		services := relay.Services().List()
		w.Header().Set("Content-Type", "application/json")
		json.NewEncoder(w).Encode(services)
	})

}
