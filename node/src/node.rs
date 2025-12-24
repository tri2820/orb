use crate::message::{ControlMessage, Message, Service};
use anyhow::{anyhow, Result};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::sync::{mpsc, Mutex};

/// Represents a bridge connection from Relay to a target service
pub struct Bridge {
    pub bridge_id: String,
    pub service: Service,
    pub write_tx: mpsc::Sender<Vec<u8>>,
}

pub type NodeId = String;

/// The ORB Node implementation
pub struct Node {
    pub node_id: NodeId,
    pub announced_services: Arc<Mutex<Vec<Service>>>,
    pub bridges: Arc<Mutex<HashMap<String, Bridge>>>,
    pub relay_stream: Arc<Mutex<Option<TcpStream>>>,
    pub relay_tx: Arc<Mutex<Option<mpsc::Sender<Message>>>>,
}

impl Node {
    /// Create a new Node with the given ID
    pub fn new(node_id: NodeId) -> Self {
        Self {
            node_id,
            announced_services: Arc::new(Mutex::new(Vec::new())),
            bridges: Arc::new(Mutex::new(HashMap::new())),
            relay_stream: Arc::new(Mutex::new(None)),
            relay_tx: Arc::new(Mutex::new(None)),
        }
    }

    /// Connect to the relay and start the control connection
    pub async fn connect(&self, relay_addr: &str, relay_port: u16) -> Result<()> {
        let stream = TcpStream::connect((relay_addr, relay_port)).await?;
        *self.relay_stream.lock().await = Some(stream);

        // Send REGISTER message
        self.send_register().await?;

        Ok(())
    }

    /// Announce services to the relay
    pub async fn announce(&self, services: Vec<Service>) -> Result<()> {
        let msg = Message::control(ControlMessage::Announce {
            services: services.clone(),
        });

        *self.announced_services.lock().await = services;
        self.send_message(msg).await?;

        Ok(())
    }

    /// Handle an incoming message from the relay
    pub async fn handle_message(&self, msg: Message) -> Result<()> {
        match msg {
            Message::Control { msg_id, control } => {
                match control {
                    ControlMessage::OpenBridge { bridge_id, service } => {
                        println!("[Node] Received OPEN_BRIDGE for bridge_id: {}", bridge_id);
                        self.handle_open_bridge(bridge_id.clone(), service.clone(), msg_id)
                            .await?;
                    }
                    ControlMessage::CloseBridge { bridge_id } => {
                        println!("[Node] Received CLOSE_BRIDGE for bridge_id: {}", bridge_id);
                        self.handle_close_bridge(bridge_id, msg_id).await?;
                    }
                    ControlMessage::Ack { .. } => {
                        // Ignore ACKs
                    }
                    _ => {
                        println!("[Node] Received unhandled control message: {:?}", control);
                    }
                }
            }
            Message::Data { data, .. } => {
                println!("[Node] Received DATA for bridge_id: {}, {} bytes", data.bridge_id, data.payload.len());
                self.handle_data(data.bridge_id, data.payload).await?;
            }
        }

        Ok(())
    }

    /// Process messages from the relay connection
    pub async fn process_relay_messages(&self) -> Result<()> {
        println!("[Node] Starting process_relay_messages");
        let mut relay_stream = self.relay_stream.lock().await;
        let stream = relay_stream
            .take()
            .ok_or_else(|| anyhow!("No relay connection"))?;

        println!("[Node] Got relay stream, splitting...");
        let (mut reader, mut writer) = stream.into_split();

        // Create channel for sending messages
        let (tx, mut rx) = mpsc::channel::<Message>(100);
        *self.relay_tx.lock().await = Some(tx);
        println!("[Node] Channel set up for sending messages");

        // Spawn writer task
        tokio::spawn(async move {
            println!("[Node] Writer task started");
            while let Some(msg) = rx.recv().await {
                println!("[Node] Writer task sending message to relay: {:?}", msg);
                match msg.encode() {
                    Ok(encoded) => {
                        let len = encoded.len() as u32;
                        if writer.write_all(&len.to_be_bytes()).await.is_err() {
                            eprintln!("[Node] Writer task failed to write length");
                            break;
                        }
                        if writer.write_all(&encoded).await.is_err() {
                            eprintln!("[Node] Writer task failed to write message");
                            break;
                        }
                        println!("[Node] Writer task sent message successfully");
                    }
                    Err(e) => {
                        eprintln!("[Node] Writer task failed to encode: {}", e);
                        break;
                    }
                }
            }
            println!("[Node] Writer task ended");
        });

        let self_rc = Arc::new(self.clone_for_task());

        println!("[Node] Entering message loop...");
        loop {
            // Read length prefix (4 bytes, big-endian)
            println!("[Node] Waiting to read message length...");
            let mut len_buf = [0u8; 4];
            match reader.read_exact(&mut len_buf).await {
                Ok(0) => {
                    return Err(anyhow!("Relay connection closed"));
                }
                Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => {
                    return Err(anyhow!("Relay connection closed"));
                }
                Err(e) => {
                    return Err(anyhow!("Failed to read message length: {}", e));
                }
                Ok(_) => {}
            }

            let len = u32::from_be_bytes(len_buf) as usize;
            println!("[Node] Read message length: {} bytes", len);
            if len > 1024 * 1024 {
                return Err(anyhow!("Message too large: {} bytes", len));
            }

            // Read message
            let mut buf = vec![0u8; len];
            reader.read_exact(&mut buf).await?;

            println!("[Node] Read message payload, decoding...");
            // Decode and handle message
            match Message::decode(&buf) {
                Ok(msg) => {
                    println!("[Node] Decoded message, handling...");
                    if let Err(e) = self_rc.handle_message(msg).await {
                        eprintln!("Error handling message: {}", e);
                    }
                }
                Err(e) => {
                    eprintln!("Failed to decode message: {}", e);
                }
            }
        }
    }

    // Private helper methods

    async fn send_register(&self) -> Result<()> {
        let msg = Message::control(ControlMessage::Register {
            node_id: self.node_id.clone(),
        });

        self.send_message(msg).await?;
        Ok(())
    }

    pub async fn send_message(&self, msg: Message) -> Result<()> {
        // Try to send via channel first (if process_relay_messages is running)
        let relay_tx = self.relay_tx.lock().await;
        if let Some(tx) = relay_tx.as_ref() {
            tx.send(msg).await
                .map_err(|_| anyhow!("Failed to send message via channel"))?;
            return Ok(());
        }
        drop(relay_tx);

        // Fallback to direct stream write (for initial connection)
        let encoded = msg.encode()?;
        let mut relay_stream = self.relay_stream.lock().await;

        if let Some(stream) = relay_stream.as_mut() {
            // Write length prefix (4 bytes, big-endian)
            let len = encoded.len() as u32;
            stream.write_all(&len.to_be_bytes()).await?;
            // Write message
            stream.write_all(&encoded).await?;
        } else {
            return Err(anyhow!("No relay connection"));
        }

        Ok(())
    }

    async fn handle_open_bridge(
        &self,
        bridge_id: String,
        service: Service,
        msg_id: String,
    ) -> Result<()> {
        println!("[Node] Opening bridge {} to {}:{}", bridge_id, service.addr, service.port);

        // Open TCP connection to target service
        let socket = TcpStream::connect((service.addr.clone(), service.port)).await?;
        let (mut read_half, mut write_half) = socket.into_split();

        // Create channel for sending data to the service
        let (write_tx, mut write_rx) = mpsc::channel::<Vec<u8>>(100);

        // Store bridge
        let mut bridges = self.bridges.lock().await;
        bridges.insert(
            bridge_id.clone(),
            Bridge {
                bridge_id: bridge_id.clone(),
                service: service.clone(),
                write_tx,
            },
        );
        drop(bridges);

        // Spawn task to write data to service
        let bridge_id_for_write = bridge_id.clone();
        tokio::spawn(async move {
            while let Some(data) = write_rx.recv().await {
                if let Err(e) = write_half.write_all(&data).await {
                    eprintln!("[Node] Error writing to service on bridge {}: {}", bridge_id_for_write, e);
                    break;
                }
            }
        });

        // Spawn task to read from service and send to relay
        let bridge_id_clone = bridge_id.clone();
        let node_clone = self.clone();
        tokio::spawn(async move {
            let mut buf = vec![0u8; 4096];
            loop {
                match read_half.read(&mut buf).await {
                    Ok(0) => {
                        println!("[Node] Bridge {} closed by service", bridge_id_clone);
                        break;
                    }
                    Ok(n) => {
                        // Send data back to relay
                        use crate::message::DataMessage;
                        let data_msg = Message::data(DataMessage {
                            bridge_id: bridge_id_clone.clone(),
                            payload: buf[..n].to_vec(),
                        });
                        if let Err(e) = node_clone.send_message(data_msg).await {
                            eprintln!("[Node] Failed to send data to relay: {}", e);
                            break;
                        }
                    }
                    Err(e) => {
                        eprintln!("[Node] Error reading from service: {}", e);
                        break;
                    }
                }
            }

            // Clean up bridge
            let mut bridges = node_clone.bridges.lock().await;
            bridges.remove(&bridge_id_clone);
        });

        // Send ACK
        let ack = Message::ack(msg_id);
        self.send_message(ack).await?;

        println!("[Node] Bridge {} opened successfully", bridge_id);
        Ok(())
    }

    async fn handle_close_bridge(&self, bridge_id: String, _msg_id: String) -> Result<()> {
        let mut bridges = self.bridges.lock().await;
        bridges.remove(&bridge_id);
        Ok(())
    }

    async fn handle_data(&self, bridge_id: String, payload: Vec<u8>) -> Result<()> {
        let bridges = self.bridges.lock().await;

        if let Some(bridge) = bridges.get(&bridge_id) {
            // Send data through the channel to be written to the service
            bridge.write_tx.send(payload).await
                .map_err(|_| anyhow!("Failed to send data to bridge write channel"))?;
        }

        Ok(())
    }

    fn clone_for_task(&self) -> Self {
        Self {
            node_id: self.node_id.clone(),
            announced_services: Arc::clone(&self.announced_services),
            bridges: Arc::clone(&self.bridges),
            relay_stream: Arc::clone(&self.relay_stream),
            relay_tx: Arc::clone(&self.relay_tx),
        }
    }
}

impl Clone for Node {
    fn clone(&self) -> Self {
        Self {
            node_id: self.node_id.clone(),
            announced_services: Arc::clone(&self.announced_services),
            bridges: Arc::clone(&self.bridges),
            relay_stream: Arc::clone(&self.relay_stream),
            relay_tx: Arc::clone(&self.relay_tx),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_node_creation() {
        let node = Node::new("node-001".to_string());
        assert_eq!(node.node_id, "node-001");
    }
}
