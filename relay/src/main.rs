use anyhow::Result;
use clap::Parser;
use orb_node::{ControlMessage, DataMessage, Message, ProtocolHandler};
use orb_relay::{bridge_manager::BridgeManager, http, webrtc::WebRtcBridge};
use std::sync::Arc;
use tokio::net::TcpListener;
use tokio::sync::mpsc;

#[derive(Parser)]
struct Args {
    /// HTTP port for WebRTC signaling
    #[arg(long, short = 'w', default_value = "8080")]
    webrtc_port: u16,

    /// Node control port
    #[arg(long, short = 'n', default_value = "9000")]
    node_port: u16,

    /// Node control bind address
    #[arg(long, default_value = "0.0.0.0")]
    node_addr: String,
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();

    // Create bridge manager
    let bridge_manager = Arc::new(BridgeManager::new());

    // Create empty node_shadows - nodes will register dynamically
    let node_shadows = orb_relay::node_shadow::NodeShadows::new();
    let webrtc_bridge = Arc::new(WebRtcBridge::new(node_shadows, Arc::clone(&bridge_manager)));

    // Start HTTP/WebRTC signaling server
    let webrtc_bridge_http = Arc::clone(&webrtc_bridge);
    let _http_handle = tokio::spawn(async move {
        let routes = http::routes(webrtc_bridge_http);
        warp::serve(routes)
            .run(([0, 0, 0, 0], args.webrtc_port))
            .await;
    });
    println!(
        "HTTP/WebRTC server listening on http://0.0.0.0:{}",
        args.webrtc_port
    );

    // Start node control server
    let node_listen = TcpListener::bind((args.node_addr.as_str(), args.node_port)).await?;
    println!(
        "Node control server listening on {}:{}",
        args.node_addr, args.node_port
    );

    let webrtc_for_nodes = Arc::clone(&webrtc_bridge);
    let bridge_manager_for_nodes = Arc::clone(&bridge_manager);

    // Accept incoming node connections
    loop {
        if let Ok((socket, peer_addr)) = node_listen.accept().await {
            println!("[Relay] Node connected from {}", peer_addr);

            let webrtc_clone = Arc::clone(&webrtc_for_nodes);
            let bridge_mgr_clone = Arc::clone(&bridge_manager_for_nodes);
            tokio::spawn(async move {
                handle_node_connection(socket, webrtc_clone, bridge_mgr_clone).await;
            });
        }
    }
}

async fn handle_node_connection(
    socket: tokio::net::TcpStream,
    webrtc_bridge: Arc<WebRtcBridge>,
    bridge_manager: Arc<BridgeManager>,
) {
    let protocol = ProtocolHandler::new(socket);
    let mut node_id = String::new();

    // Split the protocol handler into read and write halves
    let (read_half, write_half) = protocol.into_split();
    let mut read_half = read_half;
    let mut write_half = write_half;

    // Create a channel for sending messages to the node
    let (tx, mut rx) = mpsc::channel::<Message>(100);

    // Spawn a task to send messages from the channel to the node
    tokio::spawn(async move {
        use tokio::io::AsyncWriteExt;
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
                }
                Err(e) => {
                    eprintln!("[Relay] Failed to encode message: {}", e);
                    break;
                }
            }
        }
    });

    loop {
        // Read message manually from read_half
        use tokio::io::AsyncReadExt;
        let msg_result = async {
            let mut len_buf = [0u8; 4];
            match read_half.read_exact(&mut len_buf).await {
                Ok(0) => return Ok(None),
                Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(None),
                Err(e) => return Err(anyhow::anyhow!("Failed to read message length: {}", e)),
                Ok(_) => {}
            }

            let len = u32::from_be_bytes(len_buf) as usize;
            if len > 1024 * 1024 {
                return Err(anyhow::anyhow!("Message too large: {} bytes", len));
            }

            let mut buf = vec![0u8; len];
            read_half.read_exact(&mut buf).await?;

            let msg = Message::decode(&buf)?;
            Ok(Some(msg))
        }
        .await;

        match msg_result {
            Ok(Some(msg)) => {
                println!("[Relay] Received from node: {:?}", msg);

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
                        println!("[Relay] Node registered: {}", nid);
                        // Register node with bridge manager
                        bridge_manager.register_node(node_id.clone(), tx.clone()).await;
                    }
                    Message::Control {
                        control: ControlMessage::Announce { services },
                        ..
                    } => {
                        println!(
                            "[Relay] Received announce from {}: {} services",
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
                            eprintln!("[Relay] Failed to register services: {}", e);
                        } else {
                            println!("[Relay] Services registered and available for WebRTC");
                        }
                    }
                    Message::Data {
                        data: DataMessage { bridge_id, payload },
                        ..
                    } => {
                        // Forward data to the bridge
                        if let Err(e) = bridge_manager.handle_data(bridge_id, payload).await {
                            eprintln!("[Relay] Failed to handle data: {}", e);
                        }
                    }
                    _ => {}
                }

                // Send ACK for non-ACK messages
                if !is_ack {
                    let ack = Message::ack(msg_id.clone());
                    let _ = tx.send(ack).await;
                }
            }
            Ok(None) => {
                println!("[Relay] Node {} disconnected", node_id);
                if !node_id.is_empty() {
                    webrtc_bridge.unregister_node(&node_id).await;
                    bridge_manager.unregister_node(&node_id).await;
                }
                break;
            }
            Err(e) => {
                eprintln!("[Relay] Error receiving from node: {}", e);
                if !node_id.is_empty() {
                    webrtc_bridge.unregister_node(&node_id).await;
                    bridge_manager.unregister_node(&node_id).await;
                }
                break;
            }
        }
    }
}
