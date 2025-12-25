use anyhow::Result;
use clap::Parser;
use orb_relay::{bridge_manager::BridgeManager, http, webrtc::WebRtcBridge};
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
                if let Err(e) = bridge_mgr_clone.handle_node(socket, webrtc_clone).await {
                    eprintln!("[Relay] Node handler error: {}", e);
                }
            });
        }
    }
}
