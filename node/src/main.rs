use anyhow::{anyhow, Result};
use clap::Parser;
use orb_node::{AllowList, ControlMessage, Message, ProtocolHandler};
use std::path::PathBuf;
use tokio::net::TcpStream;

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

    // Connect to relay
    println!(
        "[Node] Connecting to relay at {}:{}",
        args.relay_addr, args.relay_port
    );
    let stream = TcpStream::connect((args.relay_addr.as_str(), args.relay_port)).await?;
    let mut protocol = ProtocolHandler::new(stream);
    println!("[Node] Connected to relay");

    // Send REGISTER
    let register = Message::control(ControlMessage::Register {
        node_id: node_id.clone(),
    });
    protocol.send(&register).await?;
    println!("[Node] Sent REGISTER as '{}'", node_id);

    // Wait for ACK to REGISTER before sending ANNOUNCE
    loop {
        match protocol.recv().await {
            Ok(Some(msg)) => {
                match &msg {
                    Message::Control { control: ControlMessage::Ack { .. }, .. } => {
                        println!("[Node] Received REGISTER ACK");
                        break;
                    }
                    _ => {
                        println!("[Node] Received unexpected message before ACK: {:?}", msg);
                    }
                }
            }
            Ok(None) => {
                eprintln!("[Node] Relay closed connection before ACK");
                return Ok(());
            }
            Err(e) => {
                eprintln!("[Node] Error waiting for ACK: {}", e);
                return Ok(());
            }
        }
    }

    // Send ANNOUNCE
    println!(
        "[Node] Announcing services: {:?}",
        allowlist.services.iter().map(|s| &s.id).collect::<Vec<_>>()
    );
    let announce = Message::control(ControlMessage::Announce {
        services: allowlist.services,
    });
    protocol.send(&announce).await?;
    println!("[Node] Sent ANNOUNCE");

    // Listen for messages from relay
    println!("[Node] Listening for messages from relay (Ctrl+C to quit)");
    loop {
        match protocol.recv().await {
            Ok(Some(msg)) => {
                println!("[Node] Received: {:?}", msg);

                // Send ACK for control messages (but not ACKs themselves)
                if matches!(msg, Message::Control { control: ControlMessage::Ack { .. }, .. }) {
                    // Don't ACK ACKs - would cause infinite loop
                } else if let Message::Control { .. } = msg {
                    let ack = Message::ack(msg.msg_id().to_string());
                    if let Err(e) = protocol.send(&ack).await {
                        eprintln!("[Node] Failed to send ACK: {}", e);
                    }
                }
            }
            Ok(None) => {
                println!("[Node] Relay closed connection");
                break;
            }
            Err(e) => {
                eprintln!("[Node] Error: {}", e);
                break;
            }
        }
    }

    Ok(())
}
