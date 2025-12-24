use anyhow::Result;
use orb_node::{Auth, ControlMessage, DataMessage, Message, Service};
use orb_relay::{bridge_manager::BridgeManager, webrtc::WebRtcBridge};
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::sync::mpsc;
use tokio::time::{timeout, Duration};

/// Test RTSP streaming through bridge with real camera
/// This test connects to the actual RTSP camera and verifies data flows
#[tokio::test]
#[ignore] // Run with: cargo test rtsp_stream_test -- --ignored --nocapture
async fn test_rtsp_bridge_with_real_camera() -> Result<()> {
    println!("\n===========================================");
    println!("Testing RTSP Bridge with Real Camera");
    println!("===========================================\n");

    // Create bridge manager and WebRTC bridge
    let bridge_manager = Arc::new(BridgeManager::new());
    let node_shadows = orb_relay::node_shadow::NodeShadows::new();
    let webrtc_bridge = Arc::new(WebRtcBridge::new(node_shadows, Arc::clone(&bridge_manager)));

    // Start relay node control server
    let relay_listener = TcpListener::bind("127.0.0.1:0").await?;
    let relay_addr = relay_listener.local_addr()?;
    println!("[Test] Relay listening on {}", relay_addr);

    // Spawn relay handler
    let bridge_mgr_clone = Arc::clone(&bridge_manager);
    let webrtc_clone = Arc::clone(&webrtc_bridge);
    tokio::spawn(async move {
        if let Ok((socket, _)) = relay_listener.accept().await {
            println!("[Relay] Node connected");
            handle_node_connection(socket, webrtc_clone, bridge_mgr_clone).await;
        }
    });

    // Create and connect node
    let node = orb_node::Node::new("rtsp-test-node".to_string());
    node.connect(&relay_addr.ip().to_string(), relay_addr.port())
        .await?;
    println!("[Node] Connected to relay");

    // Spawn node message handler
    let node_clone = node.clone();
    tokio::spawn(async move {
        if let Err(e) = node_clone.process_relay_messages().await {
            eprintln!("[Node] Error processing messages: {}", e);
        }
    });

    // Give handler time to start
    tokio::time::sleep(Duration::from_millis(50)).await;

    // Real camera service from config
    let camera_service = Service {
        id: "rtsp-camera".to_string(),
        svc_type: "rtsp".to_string(),
        addr: "192.168.178.65".to_string(),
        port: 554,
        path: "/stream2".to_string(),
        auth: Some(Auth::UsernameAndPassword {
            username: "vantri".to_string(),
            password: "CHth0717ic".to_string(),
        }),
    };

    // Announce the camera service
    node.announce(vec![camera_service.clone()]).await?;
    println!("[Node] Announced RTSP camera service");

    // Register with WebRTC bridge
    webrtc_bridge
        .register_services("rtsp-test-node", vec![camera_service.clone()])
        .await?;
    println!("[Test] Service registered with WebRTC bridge");

    tokio::time::sleep(Duration::from_millis(100)).await;

    // Create a bridge to the camera
    println!("\n[Test] Creating bridge to camera...");
    let bridge_stream = bridge_manager
        .create_bridge("rtsp-test-node", &camera_service)
        .await?;
    println!("[Test] ✓ Bridge created: {}", bridge_stream.bridge_id());

    // Split the bridge stream for reading and writing
    let (mut read_half, mut write_half) = tokio::io::split(bridge_stream);

    // Send RTSP DESCRIBE request
    println!("\n[Test] Sending RTSP DESCRIBE request...");
    use base64::{engine::general_purpose, Engine as _};
    let auth_b64 = general_purpose::STANDARD.encode("vantri:CHth0717ic");
    let describe_request = format!(
        "DESCRIBE rtsp://192.168.178.65:554/stream2 RTSP/1.0\r\n\
         CSeq: 1\r\n\
         User-Agent: RustRTSPClient\r\n\
         Accept: application/sdp\r\n\
         Authorization: Basic {}\r\n\
         \r\n",
        auth_b64
    );
    write_half.write_all(describe_request.as_bytes()).await?;
    println!("[Test] ✓ DESCRIBE request sent");

    // Read RTSP response
    println!("[Test] Waiting for RTSP response...");
    let mut response_buf = vec![0u8; 4096];
    let bytes_read = timeout(Duration::from_secs(5), read_half.read(&mut response_buf))
        .await
        .map_err(|_| anyhow::anyhow!("Timeout waiting for RTSP response"))??;

    if bytes_read == 0 {
        return Err(anyhow::anyhow!("Connection closed by camera"));
    }

    let response = String::from_utf8_lossy(&response_buf[..bytes_read]);
    println!("\n[Test] ✓ Received RTSP response ({} bytes):", bytes_read);
    println!("--- Response Start ---");
    println!("{}", response);
    println!("--- Response End ---\n");

    // Check if response is valid RTSP
    if !response.starts_with("RTSP/1.0") {
        return Err(anyhow::anyhow!(
            "Invalid RTSP response: {}",
            response.lines().next().unwrap_or("empty")
        ));
    }

    // Check for SDP content
    let has_sdp = response.contains("v=0") && response.contains("m=video");
    println!("[Test] SDP present in response: {}", has_sdp);

    // Parse status code
    if let Some(status_line) = response.lines().next() {
        if status_line.contains("200") {
            println!("[Test] ✓ RTSP DESCRIBE succeeded (200 OK)");
        } else {
            println!("[Test] ⚠ RTSP response: {}", status_line);
        }
    }

    // Now let's test sending SETUP and PLAY to get actual RTP data
    println!("\n[Test] Sending RTSP SETUP request...");

    // Extract control URL from SDP
    let control_url =
        if let Some(control_line) = response.lines().find(|l| l.contains("a=control:")) {
            control_line
                .split("a=control:")
                .nth(1)
                .unwrap_or("track1")
                .trim()
        } else {
            "track1"
        };
    println!("[Test] Using control URL: {}", control_url);

    let setup_request = format!(
        "SETUP rtsp://192.168.178.65:554/stream2/{} RTSP/1.0\r\n\
         CSeq: 2\r\n\
         User-Agent: RustRTSPClient\r\n\
         Transport: RTP/AVP/TCP;interleaved=0-1\r\n\
         Authorization: Basic {}\r\n\
         \r\n",
        control_url, auth_b64
    );
    write_half.write_all(setup_request.as_bytes()).await?;
    println!("[Test] ✓ SETUP request sent");

    // Read SETUP response
    let bytes_read = timeout(Duration::from_secs(5), read_half.read(&mut response_buf))
        .await
        .map_err(|_| anyhow::anyhow!("Timeout waiting for SETUP response"))??;

    let setup_response = String::from_utf8_lossy(&response_buf[..bytes_read]);
    println!("\n[Test] SETUP response ({} bytes):", bytes_read);
    println!("{}", setup_response);

    // Extract session ID
    let session_id =
        if let Some(session_line) = setup_response.lines().find(|l| l.starts_with("Session:")) {
            session_line
                .split(':')
                .nth(1)
                .unwrap_or("")
                .split(';')
                .next()
                .unwrap_or("")
                .trim()
        } else {
            return Err(anyhow::anyhow!("No Session ID in SETUP response"));
        };
    println!("\n[Test] Session ID: {}", session_id);

    // Send PLAY request
    println!("\n[Test] Sending RTSP PLAY request...");
    let play_request = format!(
        "PLAY rtsp://192.168.178.65:554/stream2 RTSP/1.0\r\n\
         CSeq: 3\r\n\
         User-Agent: RustRTSPClient\r\n\
         Session: {}\r\n\
         Range: npt=0.000-\r\n\
         Authorization: Basic {}\r\n\
         \r\n",
        session_id, auth_b64
    );
    write_half.write_all(play_request.as_bytes()).await?;
    println!("[Test] ✓ PLAY request sent");

    // Read PLAY response
    let bytes_read = timeout(Duration::from_secs(5), read_half.read(&mut response_buf))
        .await
        .map_err(|_| anyhow::anyhow!("Timeout waiting for PLAY response"))??;

    let play_response = String::from_utf8_lossy(&response_buf[..bytes_read]);
    println!("\n[Test] PLAY response ({} bytes):", bytes_read);
    println!("{}", play_response);

    // Now read RTP data
    println!("\n[Test] Reading RTP data from camera...");
    let mut rtp_packet_count = 0;
    let mut total_bytes = 0;

    for i in 0..10 {
        match timeout(Duration::from_secs(2), read_half.read(&mut response_buf)).await {
            Ok(Ok(0)) => {
                println!("[Test] Connection closed");
                break;
            }
            Ok(Ok(bytes_read)) => {
                total_bytes += bytes_read;

                // Check if this is RTP data (starts with $ for interleaved RTP)
                if response_buf[0] == b'$' && bytes_read >= 4 {
                    let channel = response_buf[1];
                    let length = u16::from_be_bytes([response_buf[2], response_buf[3]]) as usize;
                    rtp_packet_count += 1;

                    if i == 0 {
                        println!(
                            "[Test] First RTP packet: channel={}, length={}",
                            channel, length
                        );
                        // Dump first few bytes
                        println!(
                            "[Test] First 16 bytes: {:02x?}",
                            &response_buf[4..std::cmp::min(20, bytes_read)]
                        );
                    }
                } else {
                    println!("[Test] Read {} bytes (not RTP framed)", bytes_read);
                }
            }
            Ok(Err(e)) => {
                println!("[Test] Read error: {}", e);
                break;
            }
            Err(_) => {
                if i == 0 {
                    println!("[Test] Timeout waiting for RTP data");
                }
                break;
            }
        }
    }

    println!("\n[Test] ===========================================");
    println!("[Test] RTP Data Summary:");
    println!("[Test] - Total RTP packets received: {}", rtp_packet_count);
    println!("[Test] - Total bytes received: {}", total_bytes);
    println!("[Test] ===========================================\n");

    if rtp_packet_count > 0 {
        println!("[Test] ✅ SUCCESS: Received RTP data from camera through bridge!");
    } else {
        println!("[Test] ⚠️  WARNING: No RTP data received (but RTSP handshake worked)");
    }

    Ok(())
}

// Helper function to handle node connections
async fn handle_node_connection(
    socket: tokio::net::TcpStream,
    webrtc_bridge: Arc<WebRtcBridge>,
    bridge_manager: Arc<BridgeManager>,
) {
    let mut node_id = String::new();

    let (read_half, write_half) = socket.into_split();
    let mut read_half = read_half;
    let mut write_half = write_half;

    let (tx, mut rx) = mpsc::channel::<Message>(100);

    tokio::spawn(async move {
        use tokio::io::AsyncWriteExt;
        println!("[Relay Handler] Sender task started");
        while let Some(msg) = rx.recv().await {
            println!("[Relay Handler] Sending to node: {:?}", msg);
            match msg.encode() {
                Ok(encoded) => {
                    let len = encoded.len() as u32;
                    if write_half.write_all(&len.to_be_bytes()).await.is_err() {
                        eprintln!("[Relay Handler] Failed to write length");
                        break;
                    }
                    if write_half.write_all(&encoded).await.is_err() {
                        eprintln!("[Relay Handler] Failed to write message");
                        break;
                    }
                    if write_half.flush().await.is_err() {
                        eprintln!("[Relay Handler] Failed to flush");
                        break;
                    }
                    println!("[Relay Handler] Message sent and flushed");
                }
                Err(e) => {
                    eprintln!("[Relay Handler] Encode error: {}", e);
                    break;
                }
            }
        }
        println!("[Relay Handler] Sender task ended");
    });

    println!("[Relay Handler] Entering message loop");
    loop {
        use tokio::io::AsyncReadExt;
        let msg_result = async {
            let mut len_buf = [0u8; 4];
            match read_half.read_exact(&mut len_buf).await {
                Ok(0) => return Ok(None),
                Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(None),
                Err(e) => return Err(anyhow::anyhow!("Failed to read: {}", e)),
                Ok(_) => {}
            }

            let len = u32::from_be_bytes(len_buf) as usize;
            if len > 1024 * 1024 {
                return Err(anyhow::anyhow!("Message too large"));
            }

            let mut buf = vec![0u8; len];
            read_half.read_exact(&mut buf).await?;

            let msg = Message::decode(&buf)?;
            Ok(Some(msg))
        }
        .await;

        match msg_result {
            Ok(Some(msg)) => {
                println!("[Relay Handler] Received from node: {:?}", msg);
                let is_ack = matches!(
                    msg,
                    Message::Control {
                        control: ControlMessage::Ack { .. },
                        ..
                    }
                );
                let msg_id = msg.msg_id().to_string();

                match msg {
                    Message::Control {
                        control: ControlMessage::Register { node_id: nid },
                        ..
                    } => {
                        node_id = nid.clone();
                        println!("[Relay Handler] Registering node: {}", node_id);
                        bridge_manager
                            .register_node(node_id.clone(), tx.clone())
                            .await;
                    }
                    Message::Control {
                        control: ControlMessage::Announce { services },
                        ..
                    } => {
                        println!(
                            "[Relay Handler] Received ANNOUNCE with {} services",
                            services.len()
                        );
                        let _ = webrtc_bridge.register_services(&node_id, services).await;
                    }
                    Message::Data {
                        data: DataMessage { bridge_id, payload },
                        ..
                    } => {
                        println!(
                            "[Relay Handler] Received DATA: bridge_id={}, {} bytes",
                            bridge_id,
                            payload.len()
                        );
                        let _ = bridge_manager.handle_data(bridge_id, payload).await;
                    }
                    _ => {}
                }

                if !is_ack {
                    let ack = Message::ack(msg_id);
                    let _ = tx.send(ack).await;
                }
            }
            Ok(None) => {
                println!("[Relay Handler] Node disconnected");
                if !node_id.is_empty() {
                    webrtc_bridge.unregister_node(&node_id).await;
                    bridge_manager.unregister_node(&node_id).await;
                }
                break;
            }
            Err(e) => {
                eprintln!("[Relay Handler] Error: {}", e);
                if !node_id.is_empty() {
                    webrtc_bridge.unregister_node(&node_id).await;
                    bridge_manager.unregister_node(&node_id).await;
                }
                break;
            }
        }
    }
    println!("[Relay Handler] Handler finished");
}
