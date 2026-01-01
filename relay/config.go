package main

import (
	"encoding/json"
	"os"

	"github.com/tri/orb/node"
)

// Config is the relay server configuration
type Config struct {
	Services []node.Service `json:"services"`
}

// LoadConfig loads a config from a JSON file
func LoadConfig(path string) (*Config, error) {
	data, err := os.ReadFile(path)
	if err != nil {
		return nil, err
	}
	var config Config
	if err := json.Unmarshal(data, &config); err != nil {
		return nil, err
	}
	return &config, nil
}
