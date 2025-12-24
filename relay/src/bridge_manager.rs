use anyhow::{anyhow, Result};
use orb_node::{ControlMessage, Message, Service};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{mpsc, RwLock};
use uuid::Uuid;

use crate::bridge_stream::BridgeStream;

/// Manages bridge connections between relay and nodes
pub struct BridgeManager {
    /// Map of bridge_id -> channels for data routing
    bridges: Arc<RwLock<HashMap<String, BridgeChannels>>>,
    /// Map of node_id -> protocol sender
    nodes: Arc<RwLock<HashMap<String, mpsc::Sender<Message>>>>,
}

struct BridgeChannels {
    /// Send data to the protocol client (e.g., RTSP client)
    to_client_tx: mpsc::Sender<Vec<u8>>,
}

impl BridgeManager {
    pub fn new() -> Self {
        Self {
            bridges: Arc::new(RwLock::new(HashMap::new())),
            nodes: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Register a node's control connection
    pub async fn register_node(&self, node_id: String, tx: mpsc::Sender<Message>) {
        let mut nodes = self.nodes.write().await;
        nodes.insert(node_id.clone(), tx);
        println!("[BridgeManager] Registered node: {}", node_id);
    }

    /// Unregister a node when it disconnects
    pub async fn unregister_node(&self, node_id: &str) {
        let mut nodes = self.nodes.write().await;
        nodes.remove(node_id);
        println!("[BridgeManager] Unregistered node: {}", node_id);
    }

    /// Create a new bridge connection for a service
    ///
    /// Returns a BridgeStream that can be used by protocol clients (like RTSP)
    pub async fn create_bridge(
        &self,
        node_id: &str,
        service: &Service,
    ) -> Result<BridgeStream> {
        let bridge_id = Uuid::new_v4().to_string();

        // Get the node's protocol sender
        let nodes = self.nodes.read().await;
        let node_tx = nodes
            .get(node_id)
            .ok_or_else(|| anyhow!("Node not found: {}", node_id))?
            .clone();
        drop(nodes);

        // Create channels for bidirectional communication
        let (to_client_tx, to_client_rx) = mpsc::channel::<Vec<u8>>(100);
        let (to_node_tx, mut to_node_rx) = mpsc::channel::<Message>(100);

        // Register the bridge
        {
            let mut bridges = self.bridges.write().await;
            bridges.insert(
                bridge_id.clone(),
                BridgeChannels {
                    to_client_tx: to_client_tx.clone(),
                },
            );
        }

        // Send OPEN_BRIDGE to node
        let open_msg = Message::Control {
            msg_id: Uuid::new_v4().to_string(),
            control: ControlMessage::OpenBridge {
                bridge_id: bridge_id.clone(),
                service: service.clone(),
            },
        };

        node_tx
            .send(open_msg)
            .await
            .map_err(|_| anyhow!("Failed to send OPEN_BRIDGE to node"))?;

        println!("[BridgeManager] Created bridge {} for service {} on node {}",
                 bridge_id, service.id, node_id);

        // Spawn a task to forward messages from BridgeStream to Node
        let node_tx_clone = node_tx.clone();
        let bridge_id_for_task = bridge_id.clone();
        tokio::spawn(async move {
            while let Some(msg) = to_node_rx.recv().await {
                if node_tx_clone.send(msg).await.is_err() {
                    eprintln!("[BridgeManager] Failed to forward message to node for bridge {}", bridge_id_for_task);
                    break;
                }
            }
        });

        // Create and return BridgeStream
        Ok(BridgeStream::new(bridge_id, to_node_tx, to_client_rx))
    }

    /// Handle incoming DATA message from a node
    pub async fn handle_data(&self, bridge_id: String, payload: Vec<u8>) -> Result<()> {
        let bridges = self.bridges.read().await;
        if let Some(bridge) = bridges.get(&bridge_id) {
            bridge
                .to_client_tx
                .send(payload)
                .await
                .map_err(|_| anyhow!("Failed to send data to client"))?;
        } else {
            eprintln!("[BridgeManager] Received data for unknown bridge: {}", bridge_id);
        }
        Ok(())
    }

    /// Close a bridge
    pub async fn close_bridge(&self, bridge_id: &str, node_id: &str) -> Result<()> {
        // Remove from bridges map
        {
            let mut bridges = self.bridges.write().await;
            bridges.remove(bridge_id);
        }

        // Send CLOSE_BRIDGE to node
        let nodes = self.nodes.read().await;
        if let Some(node_tx) = nodes.get(node_id) {
            let close_msg = Message::Control {
                msg_id: Uuid::new_v4().to_string(),
                control: ControlMessage::CloseBridge {
                    bridge_id: bridge_id.to_string(),
                },
            };
            let _ = node_tx.send(close_msg).await;
        }

        println!("[BridgeManager] Closed bridge {}", bridge_id);
        Ok(())
    }
}

impl Default for BridgeManager {
    fn default() -> Self {
        Self::new()
    }
}
