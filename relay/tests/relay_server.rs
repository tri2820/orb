use anyhow::Result;
use orb_relay::{http, node_shadow, webrtc::WebRtcBridge};
use std::sync::Arc;

/// Manual test to start the relay server for browser testing
/// Run with: cargo test relay_server -- --ignored --nocapture
/// Then open http://localhost:8080 in your browser to test camera connections
#[tokio::test]
#[ignore]
async fn relay_server() -> Result<()> {
    let node_shadows = node_shadow::create_example_nodes();

    // Create WebRTC bridge with known services
    let webrtc_bridge = Arc::new(WebRtcBridge::new(node_shadows));

    // Start HTTP/WebRTC signaling server
    let routes = http::routes(webrtc_bridge);
    let port = 8080u16;

    println!(
        "\n✓ HTTP/WebRTC server listening on http://0.0.0.0:{}",
        port
    );
    println!("✓ Open http://localhost:{} in your browser", port);
    println!("✓ Press Ctrl+C to stop the server\n");

    // Run the server - this will block until interrupted
    warp::serve(routes).run(([0, 0, 0, 0], port)).await;

    println!("Relay server stopped");
    Ok(())
}
