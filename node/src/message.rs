use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};
use std::fs;

/// Configuration structures for services
#[derive(Deserialize, Serialize, Debug, Clone)]
pub struct Config {
    pub services: Vec<Service>,
}

/// Authentication configuration for a service
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum Auth {
    #[serde(rename = "username_and_password")]
    UsernameAndPassword { username: String, password: String },
}

/// Represents a Service - works for both config and protocol
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Service {
    pub id: String,
    #[serde(alias = "type")]
    pub svc_type: String,
    pub addr: String,
    pub port: u16,
    #[serde(default)]
    pub path: String,
    pub auth: Option<Auth>,
}

/// Control messages in the ORB Protocol
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum ControlMessage {
    #[serde(rename = "register")]
    Register { node_id: String, msg_id: String },
    #[serde(rename = "announce")]
    Announce {
        services: Vec<Service>,
        msg_id: String,
    },
    #[serde(rename = "ack")]
    Ack { msg_id: Option<String> },
    #[serde(rename = "open_bridge")]
    OpenBridge {
        bridge_id: String,
        service: Service,
        msg_id: String,
    },
    #[serde(rename = "close_bridge")]
    CloseBridge { bridge_id: String, msg_id: String },
}

/// Data messages in the ORB Protocol
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DataMessage {
    pub bridge_id: String,
    #[serde(with = "serde_bytes")]
    pub payload: Vec<u8>,
}

/// Top-level message wrapper
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Message {
    #[serde(rename = "control")]
    Control { control: ControlMessage },
    #[serde(rename = "data")]
    Data { data: DataMessage },
}

impl Message {
    pub fn encode(&self) -> Result<Vec<u8>> {
        serde_cbor::to_vec(self).map_err(|e| anyhow!("CBOR encoding error: {}", e))
    }

    pub fn decode(buf: &[u8]) -> Result<Self> {
        serde_cbor::from_slice(buf).map_err(|e| anyhow!("CBOR decoding error: {}", e))
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_register_message() {
        let msg = Message::Control {
            control: ControlMessage::Register {
                node_id: "node-001".to_string(),
                msg_id: "msg-001".to_string(),
            },
        };

        let encoded = msg.encode().unwrap();
        let decoded = Message::decode(&encoded).unwrap();

        match decoded {
            Message::Control { control } => {
                if let ControlMessage::Register { node_id, msg_id } = control {
                    assert_eq!(node_id, "node-001");
                    assert_eq!(msg_id, "msg-001");
                } else {
                    panic!("Wrong control message type");
                }
            }
            _ => panic!("Wrong message type"),
        }
    }

    #[test]
    fn test_data_message() {
        let msg = Message::Data {
            data: DataMessage {
                bridge_id: "bridge-123".to_string(),
                payload: vec![1, 2, 3, 4, 5],
            },
        };

        let encoded = msg.encode().unwrap();
        let decoded = Message::decode(&encoded).unwrap();

        match decoded {
            Message::Data { data } => {
                assert_eq!(data.bridge_id, "bridge-123");
                assert_eq!(data.payload, vec![1, 2, 3, 4, 5]);
            }
            _ => panic!("Wrong message type"),
        }
    }
}
