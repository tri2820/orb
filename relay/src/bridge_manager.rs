use anyhow::{anyhow, Result};
use orb_node::{ControlMessage, DataMessage, Message, Service};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::sync::{mpsc, RwLock};
use uuid::Uuid;

use crate::bridge_stream::BridgeStream;
use crate::webrtc::WebRtcBridge;

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

    /// Handle a node connection - all protocol logic lives here.
    ///
    /// This method:
    /// 1. Splits the socket into read/write halves
    /// 2. Creates a writer task with auto-flush (prevents the "silent bug")
    /// 3. Reads and handles messages (Register, Announce, Data, etc.)
    /// 4. Sends ACKs for non-ACK messages
    pub async fn handle_node(
        &self,
        socket: TcpStream,
        webrtc_bridge: Arc<WebRtcBridge>,
    ) -> Result<()> {
        let mut node_id = String::new();

        // Split socket into read/write halves
        let (read_half, write_half) = socket.into_split();
        let mut read_half = tokio::io::BufReader::new(read_half);
        let mut write_half = tokio::io::BufWriter::new(write_half);

        // Create channel for sending messages to the node
        let (tx, mut rx) = mpsc::channel::<Message>(100);

        // Spawn writer task with auto-flush
        // Flush after every write for protocol reliability (prevents "silent bug")
        tokio::spawn(async move {
            while let Some(msg) = rx.recv().await {
                match msg.encode() {
                    Ok(encoded) => {
                        let len = encoded.len() as u32;
                        if write_half.write_all(&len.to_be_bytes()).await.is_err() {
                            break;
                        }
                        if write_half.write_all(&encoded).await.is_err() {
                            break;
                        }
                        if write_half.flush().await.is_err() {
                            break;
                        }
                    }
                    Err(e) => {
                        eprintln!("[BridgeManager] Failed to encode: {}", e);
                        break;
                    }
                }
            }
        });

        // Message loop
        loop {
            let msg_result = Self::read_message(&mut read_half).await;

            match msg_result {
                Ok(Some(msg)) => {
                    // Don't send ACK for ACK messages (would cause infinite loop)
                    let is_ack = matches!(msg, Message::Control { control: ControlMessage::Ack { .. }, .. });

                    // Extract msg_id from message for ACK
                    let msg_id = msg.msg_id().to_string();

                    match msg {
                        Message::Control {
                            control: ControlMessage::Register { node_id: nid },
                            ..
                        } => {
                            node_id = nid.clone();
                            println!("[BridgeManager] Node registered: {}", nid);
                            self.register_node(node_id.clone(), tx.clone()).await;
                        }
                        Message::Control {
                            control: ControlMessage::Announce { services },
                            ..
                        } => {
                            println!(
                                "[BridgeManager] Received announce from {}: {} services",
                                node_id,
                                services.len()
                            );

                            for svc in &services {
                                println!(
                                    "  - Service: {} | Type: {} | Addr: {}:{}",
                                    svc.id, svc.svc_type, svc.addr, svc.port
                                );
                            }

                            // Register services with WebRTC bridge
                            if let Err(e) = webrtc_bridge.register_services(&node_id, services).await {
                                eprintln!("[BridgeManager] Failed to register services: {}", e);
                            } else {
                                println!("[BridgeManager] Services registered and available for WebRTC");
                            }
                        }
                        Message::Data {
                            data: DataMessage { bridge_id, payload },
                            ..
                        } => {
                            // Forward data to the bridge
                            if let Err(e) = self.handle_data(bridge_id, payload).await {
                                eprintln!("[BridgeManager] Failed to handle data: {}", e);
                            }
                        }
                        _ => {}
                    }

                    // Send ACK for non-ACK messages
                    if !is_ack {
                        let ack = Message::ack(msg_id);
                        let _ = tx.send(ack).await;
                    }
                }
                Ok(None) => {
                    println!("[BridgeManager] Node {} disconnected", node_id);
                    if !node_id.is_empty() {
                        webrtc_bridge.unregister_node(&node_id).await;
                        self.unregister_node(&node_id).await;
                    }
                    break;
                }
                Err(e) => {
                    eprintln!("[BridgeManager] Error receiving from node: {}", e);
                    if !node_id.is_empty() {
                        webrtc_bridge.unregister_node(&node_id).await;
                        self.unregister_node(&node_id).await;
                    }
                    break;
                }
            }
        }

        Ok(())
    }

    /// Read a single length-prefixed message from a reader
    async fn read_message<R: AsyncReadExt + Unpin>(reader: &mut R) -> Result<Option<Message>> {
        let mut len_buf = [0u8; 4];
        match reader.read_exact(&mut len_buf).await {
            Ok(0) => return Ok(None),
            Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(None),
            Err(e) => return Err(anyhow!("Failed to read message length: {}", e)),
            Ok(_) => {}
        }

        let len = u32::from_be_bytes(len_buf) as usize;
        if len > 1024 * 1024 {
            return Err(anyhow!("Message too large: {} bytes", len));
        }

        let mut buf = vec![0u8; len];
        reader.read_exact(&mut buf).await?;

        let msg = Message::decode(&buf)?;
        Ok(Some(msg))
    }
}

impl Default for BridgeManager {
    fn default() -> Self {
        Self::new()
    }
}
