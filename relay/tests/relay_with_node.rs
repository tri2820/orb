use anyhow::Result;
use orb_node::{ControlMessage, DataMessage, Message, ProtocolHandler};
use orb_relay::{bridge_manager::BridgeManager, http, webrtc::WebRtcBridge};
use std::sync::Arc;
use tokio::net::TcpListener;
use tokio::sync::mpsc;

/// Node that connects to relay and announces services from config
async fn run_node_with_config(relay_addr: &str, relay_port: u16) -> Result<()> {
    let node_shadows = orb_relay::node_shadow::create_example_nodes();
    let node_shadow = node_shadows
        .get("example-node")
        .expect("example-node not found in config");

    println!(
        "[Node] Connecting to relay at {}:{}",
        relay_addr, relay_port
    );

    // Create a proper Node instance
    let node = orb_node::Node::new("config-node".to_string());

    // Connect to relay
    node.connect(relay_addr, relay_port).await?;
    println!("[Node] ✓ Sent REGISTER");

    // Wait a bit for ACK
    tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

    // Announce services
    node.announce(node_shadow.services.clone()).await?;
    println!(
        "[Node] ✓ Sent ANNOUNCE with {} services",
        node_shadow.services.len()
    );

    // Spawn task to handle incoming messages from relay
    tokio::spawn(async move {
        if let Err(e) = node.process_relay_messages().await {
            eprintln!("[Node] Error processing messages: {}", e);
        }
        println!("[Node] Connection closed");
    });

    Ok(())
}

/// Manual test to start relay with node connection
/// Run with: cargo test relay_with_node -- --ignored --nocapture
///
/// This test demonstrates the full flow:
/// 1. Relay HTTP server (WebRTC signaling) on port 8080
/// 2. Relay node control server on port 9000
/// 3. Node connects and announces services
/// 4. Browser can connect to relay which proxies through node
#[tokio::test]
#[ignore]
async fn relay_with_node() -> Result<()> {
    println!("\n===========================================");
    println!("Starting ORB Relay with Node Bridge");
    println!("===========================================");

    // Create bridge manager and WebRTC bridge (sharing the same bridge manager)
    let bridge_manager = Arc::new(BridgeManager::new());
    let node_shadows = orb_relay::node_shadow::NodeShadows::new();
    let webrtc_bridge = Arc::new(WebRtcBridge::new(node_shadows, Arc::clone(&bridge_manager)));

    // Start relay HTTP/WebRTC server on 8080
    let webrtc_clone = Arc::clone(&webrtc_bridge);
    let http_handle = tokio::spawn(async move {
        let routes = http::routes(webrtc_clone);
        warp::serve(routes).run(([0, 0, 0, 0], 8080)).await;
    });

    println!("✓ Relay HTTP/WebRTC server on http://0.0.0.0:8080");

    // Start node control server on 9000
    let node_listen = TcpListener::bind("127.0.0.1:9000").await?;
    println!("✓ Relay node control server on 127.0.0.1:9000");

    // Clone for node handler
    let webrtc_for_nodes = Arc::clone(&webrtc_bridge);
    let bridge_manager_for_nodes = Arc::clone(&bridge_manager);

    tokio::spawn(async move {
        loop {
            if let Ok((socket, peer_addr)) = node_listen.accept().await {
                println!("\n[Relay] Node connected from {}", peer_addr);

                let webrtc_clone = Arc::clone(&webrtc_for_nodes);
                let bridge_mgr_clone = Arc::clone(&bridge_manager_for_nodes);

                tokio::spawn(async move {
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
                                Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => {
                                    return Ok(None)
                                }
                                Err(e) => {
                                    return Err(anyhow::anyhow!("Failed to read message length: {}", e))
                                }
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

                                // Extract msg_id before match to avoid borrow issues
                                let msg_id = msg.msg_id().to_string();

                                match msg {
                                    Message::Control {
                                        control: ControlMessage::Register { node_id: nid },
                                        ..
                                    } => {
                                        node_id = nid.clone();
                                        println!("[Relay] Node registered: {}", nid);
                                        // Register node with bridge manager
                                        bridge_mgr_clone.register_node(node_id.clone(), tx.clone()).await;
                                    }
                                    Message::Control {
                                        control: ControlMessage::Announce { services },
                                        ..
                                    } => {
                                        println!(
                                            "[Relay] Received announce: {} services",
                                            services.len()
                                        );

                                        // Register services with WebRTC bridge
                                        if let Err(e) =
                                            webrtc_clone.register_services(&node_id, services).await
                                        {
                                            eprintln!("[Relay] Failed to register services: {}", e);
                                        } else {
                                            println!("[Relay] ✓ Services registered and available for WebRTC");
                                        }
                                    }
                                    Message::Data {
                                        data: DataMessage { bridge_id, payload },
                                        ..
                                    } => {
                                        // Forward data to the bridge
                                        if let Err(e) = bridge_mgr_clone.handle_data(bridge_id, payload).await {
                                            eprintln!("[Relay] Failed to handle data: {}", e);
                                        }
                                    }
                                    _ => {}
                                }

                                // Send ACK
                                let ack = Message::ack(msg_id);
                                let _ = tx.send(ack).await;
                            }
                            Ok(None) => break,
                            Err(_) => break,
                        }
                    }
                    println!("[Relay] Node disconnected");
                });
            }
        }
    });

    // Start node with services from config
    run_node_with_config("127.0.0.1", 9000).await?;

    tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;

    println!("\n===========================================");
    println!("✓ Relay with Node ready!");
    println!("===========================================");
    println!("\nArchitecture:");
    println!("  Browser (WebRTC)");
    println!("       ↓");
    println!("  Relay HTTP Server (port 8080)");
    println!("       ↔ Node Control (port 9000)");
    println!("       ↓");
    println!("  Node (announces services from config.json)");
    println!("       ↓");
    println!("  Real Services (from config)");
    println!("\nTo test:");
    println!("  1. Open http://localhost:8080 in browser");
    println!("  2. Connect to services announced by node");
    println!("  3. Node proxies through to real servers");
    println!("  4. Check console for relay/node communication");
    println!("\nPress Ctrl+C to stop\n");

    // Keep running
    http_handle.await?;

    Ok(())
}
