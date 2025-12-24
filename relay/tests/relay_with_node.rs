use anyhow::Result;
use orb_node::{ControlMessage, Message, ProtocolHandler};
use orb_relay::{http, webrtc::WebRtcBridge};
use std::sync::Arc;
use tokio::net::{TcpListener, TcpStream};

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
    let stream = TcpStream::connect((relay_addr, relay_port)).await?;
    let mut protocol = ProtocolHandler::new(stream);

    // Send REGISTER
    let register = Message::control(ControlMessage::Register {
        node_id: "config-node".to_string(),
    });
    protocol.send(&register).await?;
    println!("[Node] ✓ Sent REGISTER");

    // Wait for ACK
    tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

    // Send ANNOUNCE with services from config
    let announce = Message::control(ControlMessage::Announce {
        services: node_shadow.services.clone(),
    });
    protocol.send(&announce).await?;
    println!(
        "[Node] ✓ Sent ANNOUNCE with {} services",
        node_shadow.services.len()
    );

    // Keep connection alive and listen for messages
    tokio::spawn(async move {
        loop {
            match protocol.recv().await {
                Ok(Some(msg)) => println!("[Node] Received from relay: {:?}", msg),
                Ok(None) => break,
                Err(e) => {
                    eprintln!("[Node] Error: {}", e);
                    break;
                }
            }
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

    let webrtc_bridge = Arc::new(WebRtcBridge::default());

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

    // Clone webrtc_bridge for node handler
    let webrtc_for_nodes = Arc::clone(&webrtc_bridge);

    tokio::spawn(async move {
        loop {
            if let Ok((socket, peer_addr)) = node_listen.accept().await {
                println!("\n[Relay] Node connected from {}", peer_addr);

                let webrtc_clone = Arc::clone(&webrtc_for_nodes);
                tokio::spawn(async move {
                    let mut protocol = ProtocolHandler::new(socket);
                    let mut node_id = String::new();

                    loop {
                        match protocol.recv().await {
                            Ok(Some(msg)) => {
                                println!("[Relay] Received from node: {:?}", msg);

                                // Extract msg_id before match to avoid borrow issues
                                let msg_id = msg.msg_id().to_string();

                                match msg {
                                    Message::Control {
                                        control: ControlMessage::Register { node_id: nid },
                                        ..
                                    } => {
                                        node_id = nid;
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
                                    _ => {}
                                }

                                // Send ACK
                                let ack = Message::ack(msg_id);
                                let _ = protocol.send(&ack).await;
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
