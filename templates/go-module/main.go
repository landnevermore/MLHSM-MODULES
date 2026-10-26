package main

import (
	"encoding/json"
	"log"
	"net"
	"net/http"
	"os"
	"strconv"
	"time"
)

func main() {
	port := os.Getenv("MLHSM_MODULE_PORT")
	if port == "" && len(os.Args) > 1 {
		port = os.Args[1]
	}
	if port == "" {
		log.Fatal("MLHSM_MODULE_PORT is required")
	}
	if _, err := strconv.Atoi(port); err != nil {
		log.Fatalf("invalid module port: %v", err)
	}

	mux := http.NewServeMux()
	mux.HandleFunc("GET /health", func(w http.ResponseWriter, _ *http.Request) {
		w.Header().Set("Content-Type", "application/json")
		_ = json.NewEncoder(w).Encode(map[string]string{"status": "ok"})
	})
	mux.HandleFunc("GET /api/v1/example/status", func(w http.ResponseWriter, _ *http.Request) {
		w.Header().Set("Content-Type", "application/json")
		_ = json.NewEncoder(w).Encode(map[string]string{
			"module": "example",
			"status": "ready",
		})
	})

	listener, err := net.Listen("tcp", net.JoinHostPort("127.0.0.1", port))
	if err != nil {
		log.Fatalf("listen: %v", err)
	}
	server := &http.Server{
		Handler:           mux,
		ReadHeaderTimeout: 5 * time.Second,
	}
	log.Printf("module listening on loopback port %s", port)
	if err := server.Serve(listener); err != nil && err != http.ErrServerClosed {
		log.Fatal(err)
	}
}
