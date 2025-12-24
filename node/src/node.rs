use crate::message::{ControlMessage, Message, Service};
use anyhow::{anyhow, Result};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::sync::Mutex;

/// Represents a bridge connection from Relay to a target service
#[allow(dead_code)]
pub struct Bridge {
    pub bridge_id: String,
    pub service: Service,
    pub socket: Option<TcpStream>,
}

pub type NodeId = String;

/// The ORB Node implementation
pub struct Node {
    pub node_id: NodeId,
    pub announced_services: Arc<Mutex<Vec<Service>>>,
    pub bridges: Arc<Mutex<HashMap<String, Bridge>>>,
    pub relay_stream: Arc<Mutex<Option<TcpStream>>>,
}

impl Node {
    /// Create a new Node with the given ID
    pub fn new(node_id: NodeId) -> Self {
        Self {
            node_id,
            announced_services: Arc::new(Mutex::new(Vec::new())),
            bridges: Arc::new(Mutex::new(HashMap::new())),
            relay_stream: Arc::new(Mutex::new(None)),
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
        let msg_id = uuid::Uuid::new_v4().to_string();

        let msg = Message::Control {
            control: ControlMessage::Announce {
                services: services.clone(),
                msg_id,
            },
        };

        *self.announced_services.lock().await = services;
        self.send_message(msg).await?;

        Ok(())
    }

    /// Handle an incoming message from the relay
    pub async fn handle_message(&self, msg: Message) -> Result<()> {
        match msg {
            Message::Control { control } => {
                match control {
                    ControlMessage::OpenBridge {
                        bridge_id,
                        service,
                        msg_id,
                    } => {
                        self.handle_open_bridge(bridge_id.clone(), service.clone(), msg_id)
                            .await?;
                    }
                    ControlMessage::CloseBridge { bridge_id, msg_id } => {
                        self.handle_close_bridge(bridge_id, msg_id).await?;
                    }
                    _ => {
                        // Ignore other control message types
                    }
                }
            }
            Message::Data { data } => {
                self.handle_data(data.bridge_id, data.payload).await?;
            }
        }

        Ok(())
    }

    /// Process messages from the relay connection
    pub async fn process_relay_messages(&self) -> Result<()> {
        let mut relay_stream = self.relay_stream.lock().await;
        let stream = relay_stream
            .take()
            .ok_or_else(|| anyhow!("No relay connection"))?;

        let (mut reader, _writer) = stream.into_split();

        let self_rc = Arc::new(self.clone_for_task());

        loop {
            let mut buf = vec![0; 4096];
            match reader.read(&mut buf).await {
                Ok(0) => {
                    return Err(anyhow!("Relay connection closed"));
                }
                Ok(n) => {
                    buf.truncate(n);

                    // Try to decode message
                    if let Ok(msg) = Message::decode(&buf) {
                        if let Err(e) = self_rc.handle_message(msg).await {
                            eprintln!("Error handling message: {}", e);
                        }
                    }
                }
                Err(e) => {
                    return Err(anyhow!("Relay connection error: {}", e));
                }
            }
        }
    }

    // Private helper methods

    async fn send_register(&self) -> Result<()> {
        let msg_id = uuid::Uuid::new_v4().to_string();
        let msg = Message::Control {
            control: ControlMessage::Register {
                node_id: self.node_id.clone(),
                msg_id,
            },
        };

        self.send_message(msg).await?;
        Ok(())
    }

    pub async fn send_message(&self, msg: Message) -> Result<()> {
        let encoded = msg.encode()?;
        let mut relay_stream = self.relay_stream.lock().await;

        if let Some(stream) = relay_stream.as_mut() {
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
        // Open TCP connection to target service
        let socket = TcpStream::connect((service.addr.clone(), service.port)).await?;

        // Store bridge
        let mut bridges = self.bridges.lock().await;
        bridges.insert(
            bridge_id.clone(),
            Bridge {
                bridge_id: bridge_id.clone(),
                service,
                socket: Some(socket),
            },
        );

        // Send ACK
        let ack = Message::Control {
            control: ControlMessage::Ack {
                msg_id: Some(msg_id),
            },
        };
        self.send_message(ack).await?;

        Ok(())
    }

    async fn handle_close_bridge(&self, bridge_id: String, _msg_id: String) -> Result<()> {
        let mut bridges = self.bridges.lock().await;
        bridges.remove(&bridge_id);
        Ok(())
    }

    async fn handle_data(&self, bridge_id: String, payload: Vec<u8>) -> Result<()> {
        let mut bridges = self.bridges.lock().await;

        if let Some(bridge) = bridges.get_mut(&bridge_id) {
            if let Some(socket) = &mut bridge.socket {
                socket.write_all(&payload).await?;
            }
        }

        Ok(())
    }

    fn clone_for_task(&self) -> Self {
        Self {
            node_id: self.node_id.clone(),
            announced_services: Arc::clone(&self.announced_services),
            bridges: Arc::clone(&self.bridges),
            relay_stream: Arc::clone(&self.relay_stream),
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
