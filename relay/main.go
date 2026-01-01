package main

import (
	"flag"
	"log"
	"os"
	"os/signal"
	"syscall"
)

func main() {
	addr := flag.String("addr", ":9000", "Listen address for node connections")
	httpAddr := flag.String("http", ":8080", "Listen address for HTTP API")
	flag.Parse()

	server := NewServer()

	if err := server.Listen(*addr); err != nil {
		log.Fatalf("Failed to listen: %v", err)
	}

	// Start HTTP API
	go startHTTPAPI(server, *httpAddr)

	// Handle shutdown signals
	sigChan := make(chan os.Signal, 1)
	signal.Notify(sigChan, syscall.SIGINT, syscall.SIGTERM)

	go func() {
		<-sigChan
		log.Println("[Main] Shutting down...")
		server.Shutdown()
	}()

	if err := server.Serve(); err != nil {
		log.Fatalf("Server error: %v", err)
	}
}
