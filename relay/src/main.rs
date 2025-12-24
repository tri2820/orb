use anyhow::Result;
use clap::Parser;
use orb_relay::{config_to_services, http, read_config, webrtc::WebRtcBridge};
use std::collections::HashMap;

#[derive(Parser)]
struct Args {
    /// Path to the configuration file
    #[arg(long, short = 'c', default_value = "config.json")]
    config: String,

    /// HTTP port for WebRTC signaling
    #[arg(long, short = 'p', default_value = "8080")]
    port: u16,
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();

    // Load and parse config file
    let config = read_config(&args.config)?;
    let services = config_to_services(config);

    println!("Orb Relay starting with {} services", services.len());
    for (id, service) in &services {
        println!("  - {}: {} at {}:{}", id, service.svc_type, service.ip, service.port);
    }

    // Convert services to HashMap
    let services_map: HashMap<String, orb_relay::config::Service> = services.into_iter().collect();

    // Create WebRTC bridge with known services
    let webrtc_bridge = std::sync::Arc::new(WebRtcBridge::new(services_map));

    // Start HTTP/WebRTC signaling server
    let routes = http::routes(webrtc_bridge);
    println!("HTTP/WebRTC server listening on http://0.0.0.0:{}", args.port);

    tokio::task::spawn(warp::serve(routes).run(([0, 0, 0, 0], args.port)));

    // Wait for Ctrl+C
    tokio::signal::ctrl_c().await?;
    println!("Shutting down...");

    Ok(())
}
