use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;

use anyhow::{anyhow, Result};
use orb_node::{NodeId, Service};
use serde::{Deserialize, Serialize};

pub struct NodeShadow {
    pub services: Vec<orb_node::Service>,
}

/// Configuration structures for services
#[derive(Deserialize, Serialize, Debug, Clone)]
pub struct Config {
    pub services: Vec<Service>,
}

/// Read and parse configuration from a file path
pub fn read_config<P: AsRef<std::path::Path>>(path: P) -> Result<Config> {
    let config_path = path.as_ref();
    let config_content = fs::read_to_string(config_path)
        .map_err(|e| anyhow!("Failed to read config file '{:?}': {}", config_path, e))?;

    let config: Config = serde_json::from_str(&config_content)
        .map_err(|e| anyhow!("Failed to parse config file '{:?}': {}", config_path, e))?;

    if config.services.is_empty() {
        return Err(anyhow!("No services found in config file"));
    }

    Ok(Config {
        services: config.services,
    })
}

pub type NodeShadows = HashMap<NodeId, NodeShadow>;

pub fn create_example_nodes() -> NodeShadows {
    // Load and parse config file
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let config_path = root.join("config.json");
    let config = read_config(config_path).expect("Failed to read config file");

    let mut node_shadows: NodeShadows = HashMap::new();
    node_shadows.insert(
        "example-node".to_string(),
        NodeShadow {
            services: config.services,
        },
    );

    println!("\n===========================================");
    println!("Orb Relay starting with {} nodes", node_shadows.len());
    for (id, shadow) in &node_shadows {
        for service in &shadow.services {
            println!(
                " - Node: {} | Service: {} | Type: {} | Addr: {}:{} | Auth: {:?}",
                id, service.id, service.svc_type, service.addr, service.port, service.auth
            );
        }
    }
    println!("===========================================\n");

    node_shadows
}
