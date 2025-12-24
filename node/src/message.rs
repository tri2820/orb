use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};

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

/// Allowlist configuration containing a list of services
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AllowList {
    pub services: Vec<Service>,
}

/// Control messages in the ORB Protocol
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum ControlMessage {
    #[serde(rename = "register")]
    Register { node_id: String },
    #[serde(rename = "announce")]
    Announce { services: Vec<Service> },
    #[serde(rename = "ack")]
    Ack { ack_msg_id: String },
    #[serde(rename = "open_bridge")]
    OpenBridge { bridge_id: String, service: Service },
    #[serde(rename = "close_bridge")]
    CloseBridge { bridge_id: String },
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
    Control {
        msg_id: String,
        control: ControlMessage,
    },
    #[serde(rename = "data")]
    Data { msg_id: String, data: DataMessage },
}

impl Message {
    pub fn encode(&self) -> Result<Vec<u8>> {
        serde_cbor::to_vec(self).map_err(|e| anyhow!("CBOR encoding error: {}", e))
    }

    pub fn decode(buf: &[u8]) -> Result<Self> {
        serde_cbor::from_slice(buf).map_err(|e| anyhow!("CBOR decoding error: {}", e))
    }

    /// Get msg_id from any message
    pub fn msg_id(&self) -> &str {
        match self {
            Message::Control { msg_id, .. } => msg_id,
            Message::Data { msg_id, .. } => msg_id,
        }
    }

    /// Create a control message with auto-generated msg_id
    pub fn control(control: ControlMessage) -> Self {
        Message::Control {
            msg_id: uuid::Uuid::new_v4().to_string(),
            control,
        }
    }

    /// Create a data message with auto-generated msg_id
    pub fn data(data: DataMessage) -> Self {
        Message::Data {
            msg_id: uuid::Uuid::new_v4().to_string(),
            data,
        }
    }

    /// Create an ACK message for a given message ID
    pub fn ack(ack_msg_id: String) -> Self {
        Message::Control {
            msg_id: uuid::Uuid::new_v4().to_string(),
            control: ControlMessage::Ack { ack_msg_id },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_register_message() {
        let msg = Message::Control {
            msg_id: "msg-001".to_string(),
            control: ControlMessage::Register {
                node_id: "node-001".to_string(),
            },
        };

        let encoded = msg.encode().unwrap();
        let decoded = Message::decode(&encoded).unwrap();

        match decoded {
            Message::Control { msg_id, control } => {
                assert_eq!(msg_id, "msg-001");
                if let ControlMessage::Register { node_id } = control {
                    assert_eq!(node_id, "node-001");
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
            msg_id: "msg-002".to_string(),
            data: DataMessage {
                bridge_id: "bridge-123".to_string(),
                payload: vec![1, 2, 3, 4, 5],
            },
        };

        let encoded = msg.encode().unwrap();
        let decoded = Message::decode(&encoded).unwrap();

        match decoded {
            Message::Data { msg_id, data } => {
                assert_eq!(msg_id, "msg-002");
                assert_eq!(data.bridge_id, "bridge-123");
                assert_eq!(data.payload, vec![1, 2, 3, 4, 5]);
            }
            _ => panic!("Wrong message type"),
        }
    }
}
