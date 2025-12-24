use anyhow::Result;
use clap::Parser;
use orb_relay::{http, webrtc::WebRtcBridge};

#[derive(Parser)]
struct Args {
    /// HTTP port for WebRTC signaling
    #[arg(long, short = 'p', default_value = "8080")]
    port: u16,
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();
    let node_shadows = orb_relay::node_shadow::create_example_nodes();

    // Create WebRTC bridge with known services
    let webrtc_bridge = std::sync::Arc::new(WebRtcBridge::new(node_shadows));

    // Start HTTP/WebRTC signaling server
    let routes = http::routes(webrtc_bridge);
    println!(
        "HTTP/WebRTC server listening on http://0.0.0.0:{}",
        args.port
    );

    tokio::task::spawn(warp::serve(routes).run(([0, 0, 0, 0], args.port)));

    // Wait for Ctrl+C
    tokio::signal::ctrl_c().await?;
    println!("Shutting down...");

    Ok(())
}
