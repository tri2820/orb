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

	relay := NewRelay()

	if err := relay.Listen(*addr); err != nil {
		log.Fatalf("Failed to listen: %v", err)
	}

	// Start HTTP API
	go startHTTPAPI(relay, *httpAddr)

	// Handle shutdown signals
	sigChan := make(chan os.Signal, 1)
	signal.Notify(sigChan, syscall.SIGINT, syscall.SIGTERM)

	go func() {
		<-sigChan
		log.Println("[Main] Shutting down...")
		relay.Shutdown()
	}()

	if err := relay.Serve(); err != nil {
		log.Fatalf("Relay error: %v", err)
	}
}
