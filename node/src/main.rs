use anyhow::{anyhow, Result};
use clap::Parser;
use orb_node::{AllowList, Node};
use std::path::PathBuf;

#[derive(Parser)]
struct Args {
    /// Relay server address
    #[arg(long, default_value = "127.0.0.1")]
    relay_addr: String,

    /// Relay server port
    #[arg(long, short = 'p', default_value = "9000")]
    relay_port: u16,

    /// Path to allowlist file (line-separated service configs as JSON)
    #[arg(long, short = 'a', default_value = "allowlist.json")]
    allowlist: PathBuf,
}

/// Read allowlist from file
fn read_allowlist(path: &PathBuf) -> Result<AllowList> {
    let content = std::fs::read_to_string(path)
        .map_err(|e| anyhow!("Failed to read allowlist file '{:?}': {}", path, e))?;

    serde_json::from_str(&content).map_err(|e| anyhow!("Invalid allowlist JSON: {}", e))
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();

    // Generate random node ID
    let node_id = uuid::Uuid::new_v4().to_string();

    // Read allowlist
    let allowlist = read_allowlist(&args.allowlist)?;
    println!(
        "[Node] Loaded {} services from {}",
        allowlist.services.len(),
        args.allowlist.display()
    );

    if allowlist.services.is_empty() {
        eprintln!("[Node] Error: No services to announce");
        return Ok(());
    }

    // Create node and run
    let node = Node::new(node_id);
    node.run(&args.relay_addr, args.relay_port, allowlist.services).await?;

    Ok(())
}
