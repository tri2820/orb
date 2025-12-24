use anyhow::Result;
use clap::Parser;
use orb_node::{ControlMessage, Message, ProtocolHandler};
use orb_relay::{http, webrtc::WebRtcBridge};
use std::sync::Arc;
use tokio::net::TcpListener;

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

    // Create empty node_shadows - nodes will register dynamically
    let node_shadows = orb_relay::node_shadow::NodeShadows::new();
    let webrtc_bridge = Arc::new(WebRtcBridge::new(node_shadows));

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

    // Accept incoming node connections
    loop {
        if let Ok((socket, peer_addr)) = node_listen.accept().await {
            println!("[Relay] Node connected from {}", peer_addr);

            let webrtc_clone = Arc::clone(&webrtc_for_nodes);
            tokio::spawn(async move {
                handle_node_connection(socket, webrtc_clone).await;
            });
        }
    }
}

async fn handle_node_connection(socket: tokio::net::TcpStream, webrtc_bridge: Arc<WebRtcBridge>) {
    let mut protocol = ProtocolHandler::new(socket);
    let mut node_id = String::new();

    loop {
        match protocol.recv().await {
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
                    _ => {}
                }

                // Send ACK for non-ACK messages
                if !is_ack {
                    let ack = Message::ack(msg_id.clone());
                    let _ = protocol.send(&ack).await;
                }
            }
            Ok(None) => {
                println!("[Relay] Node {} disconnected", node_id);
                if !node_id.is_empty() {
                    webrtc_bridge.unregister_node(&node_id).await;
                }
                break;
            }
            Err(e) => {
                eprintln!("[Relay] Error receiving from node: {}", e);
                if !node_id.is_empty() {
                    webrtc_bridge.unregister_node(&node_id).await;
                }
                break;
            }
        }
    }
}
