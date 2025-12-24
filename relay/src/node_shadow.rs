use std::collections::HashMap;

use orb_node::{message::read_config, NodeId};

pub struct NodeShadow {
    pub services: Vec<orb_node::Service>,
}

pub type NodeShadows = HashMap<NodeId, NodeShadow>;

pub fn create_example_nodes() -> NodeShadows {
    // Load and parse config file
    let config = read_config("config.json").expect("Failed to read config file");

    let mut node_shadows: NodeShadows = HashMap::new();
    node_shadows.insert(
        "example-node".to_string(),
        NodeShadow {
            services: config.services,
        },
    );

    println!("\n===========================================",);
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
