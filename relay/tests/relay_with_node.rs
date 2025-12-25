use anyhow::Result;
use orb_relay::{bridge_manager::BridgeManager, http, webrtc::WebRtcBridge};
use std::sync::Arc;
use tokio::net::TcpListener;

/// Node that connects to relay and announces services from config
async fn run_node_with_config(relay_addr: String, relay_port: u16) -> Result<()> {
    let node_shadows = orb_relay::node_shadow::create_example_nodes();
    let node_shadow = node_shadows
        .get("example-node")
        .expect("example-node not found in config");

    println!(
        "[Node] Connecting to relay at {}:{}",
        relay_addr, relay_port
    );

    // Clone the services before moving into the spawn
    let services = node_shadow.services.clone();

    // Create a proper Node instance with the new simplified API
    let node = orb_node::Node::new("config-node".to_string());

    // Use the new run() method that handles everything
    tokio::spawn(async move {
        if let Err(e) = node.run(&relay_addr, relay_port, services).await {
            eprintln!("[Node] Error: {}", e);
        }
        println!("[Node] Connection closed");
    });

    // Wait a bit for the node to connect
    tokio::time::sleep(tokio::time::Duration::from_millis(200)).await;

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

    // Accept node connections using the new handle_node method
    tokio::spawn(async move {
        loop {
            if let Ok((socket, peer_addr)) = node_listen.accept().await {
                println!("\n[Relay] Node connected from {}", peer_addr);

                let webrtc_clone = Arc::clone(&webrtc_for_nodes);
                let bridge_mgr_clone = Arc::clone(&bridge_manager_for_nodes);

                tokio::spawn(async move {
                    if let Err(e) = bridge_mgr_clone.handle_node(socket, webrtc_clone).await {
                        eprintln!("[Relay] Node handler error: {}", e);
                    }
                });
            }
        }
    });

    // Start node with services from config
    run_node_with_config("127.0.0.1".to_string(), 9000).await?;

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
