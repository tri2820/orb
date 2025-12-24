use anyhow::Result;
use orb_node::{ControlMessage, DataMessage, Message, Node, Service};
use orb_relay::{bridge_manager::BridgeManager, webrtc::WebRtcBridge};
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc;
use tokio::time::{timeout, Duration};

/// Test that a bridge can be created and data flows bidirectionally
#[tokio::test]
async fn test_bridge_bidirectional_data_flow() -> Result<()> {
    // Setup: Create a mock TCP service that echoes data back
    let echo_server = TcpListener::bind("127.0.0.1:0").await?;
    let echo_addr = echo_server.local_addr()?;

    // Spawn echo server
    tokio::spawn(async move {
        if let Ok((mut socket, _)) = echo_server.accept().await {
            let mut buf = vec![0u8; 1024];
            while let Ok(n) = socket.read(&mut buf).await {
                if n == 0 {
                    break;
                }
                let _ = socket.write_all(&buf[..n]).await;
            }
        }
    });

    // Create relay server
    let relay_listener = TcpListener::bind("127.0.0.1:0").await?;
    let relay_addr = relay_listener.local_addr()?;

    let bridge_manager = Arc::new(BridgeManager::new());
    let webrtc_bridge = Arc::new(WebRtcBridge::new(
        Default::default(),
        Arc::clone(&bridge_manager),
    ));

    // Spawn relay node handler
    let bridge_mgr_clone = Arc::clone(&bridge_manager);
    let webrtc_clone = Arc::clone(&webrtc_bridge);
    tokio::spawn(async move {
        println!("[Relay] Waiting for node connection...");
        if let Ok((socket, _)) = relay_listener.accept().await {
            println!("[Relay] Node connected, starting handler");
            handle_node_connection(socket, webrtc_clone, bridge_mgr_clone).await;
            println!("[Relay] Handler finished");
        }
    });

    // Create and connect node
    let node = Node::new("test-node".to_string());
    node.connect(&relay_addr.ip().to_string(), relay_addr.port())
        .await?;

    // Spawn node message handler immediately after connect
    let node_clone = node.clone();
    tokio::spawn(async move {
        let _ = node_clone.process_relay_messages().await;
    });

    // Give the message handler time to start
    tokio::time::sleep(Duration::from_millis(50)).await;

    // Announce the echo service
    let service = Service {
        id: "echo-service".to_string(),
        svc_type: "tcp".to_string(),
        addr: echo_addr.ip().to_string(),
        port: echo_addr.port(),
        path: String::new(),
        auth: None,
    };
    node.announce(vec![service.clone()]).await?;

    // Register services with WebRTC bridge
    webrtc_bridge
        .register_services("test-node", vec![service.clone()])
        .await?;

    // Give everything time to connect
    tokio::time::sleep(Duration::from_millis(100)).await;

    // Create a bridge
    let bridge_stream = bridge_manager.create_bridge("test-node", &service).await?;

    // Test: Send data through bridge
    let test_data = b"Hello, Bridge!";
    let (mut read_half, mut write_half) = tokio::io::split(bridge_stream);

    // Write data
    write_half.write_all(test_data).await?;

    // Read echoed data back
    let mut response = vec![0u8; test_data.len()];
    timeout(Duration::from_secs(2), read_half.read_exact(&mut response))
        .await
        .map_err(|_| anyhow::anyhow!("Timeout waiting for echo response"))??;

    // Verify
    assert_eq!(
        &response[..],
        test_data,
        "Echo response should match sent data"
    );

    Ok(())
}

/// Test that multiple bridges can coexist
#[tokio::test]
async fn test_multiple_bridges() -> Result<()> {
    // Setup two echo servers
    let echo1 = TcpListener::bind("127.0.0.1:0").await?;
    let echo1_addr = echo1.local_addr()?;

    let echo2 = TcpListener::bind("127.0.0.1:0").await?;
    let echo2_addr = echo2.local_addr()?;

    // Spawn echo servers
    for echo_server in [echo1, echo2] {
        tokio::spawn(async move {
            if let Ok((mut socket, _)) = echo_server.accept().await {
                let mut buf = vec![0u8; 1024];
                while let Ok(n) = socket.read(&mut buf).await {
                    if n == 0 {
                        break;
                    }
                    let _ = socket.write_all(&buf[..n]).await;
                }
            }
        });
    }

    // Create relay
    let relay_listener = TcpListener::bind("127.0.0.1:0").await?;
    let relay_addr = relay_listener.local_addr()?;

    let bridge_manager = Arc::new(BridgeManager::new());
    let webrtc_bridge = Arc::new(WebRtcBridge::new(
        Default::default(),
        Arc::clone(&bridge_manager),
    ));

    let bridge_mgr_clone = Arc::clone(&bridge_manager);
    let webrtc_clone = Arc::clone(&webrtc_bridge);
    tokio::spawn(async move {
        if let Ok((socket, _)) = relay_listener.accept().await {
            handle_node_connection(socket, webrtc_clone, bridge_mgr_clone).await;
        }
    });

    // Create node with two services
    let node = Node::new("multi-test-node".to_string());
    node.connect(&relay_addr.ip().to_string(), relay_addr.port())
        .await?;

    // Spawn node message handler immediately after connect
    let node_clone = node.clone();
    tokio::spawn(async move {
        let _ = node_clone.process_relay_messages().await;
    });

    // Give the message handler time to start
    tokio::time::sleep(Duration::from_millis(50)).await;

    let service1 = Service {
        id: "echo1".to_string(),
        svc_type: "tcp".to_string(),
        addr: echo1_addr.ip().to_string(),
        port: echo1_addr.port(),
        path: String::new(),
        auth: None,
    };

    let service2 = Service {
        id: "echo2".to_string(),
        svc_type: "tcp".to_string(),
        addr: echo2_addr.ip().to_string(),
        port: echo2_addr.port(),
        path: String::new(),
        auth: None,
    };

    node.announce(vec![service1.clone(), service2.clone()])
        .await?;

    webrtc_bridge
        .register_services("multi-test-node", vec![service1.clone(), service2.clone()])
        .await?;

    tokio::time::sleep(Duration::from_millis(100)).await;

    // Create two bridges simultaneously
    let bridge1 = bridge_manager
        .create_bridge("multi-test-node", &service1)
        .await?;
    let bridge2 = bridge_manager
        .create_bridge("multi-test-node", &service2)
        .await?;

    // Test both bridges independently
    let test1 = tokio::spawn(async move {
        let (mut read, mut write) = tokio::io::split(bridge1);
        let data = b"Bridge 1";
        write.write_all(data).await?;

        let mut response = vec![0u8; data.len()];
        timeout(Duration::from_secs(2), read.read_exact(&mut response))
            .await
            .map_err(|_| anyhow::anyhow!("Timeout"))??;

        assert_eq!(&response[..], data);
        Ok::<(), anyhow::Error>(())
    });

    let test2 = tokio::spawn(async move {
        let (mut read, mut write) = tokio::io::split(bridge2);
        let data = b"Bridge 2";
        write.write_all(data).await?;

        let mut response = vec![0u8; data.len()];
        timeout(Duration::from_secs(2), read.read_exact(&mut response))
            .await
            .map_err(|_| anyhow::anyhow!("Timeout"))??;

        assert_eq!(&response[..], data);
        Ok::<(), anyhow::Error>(())
    });

    test1.await??;
    test2.await??;

    Ok(())
}

/// Test that bridge handles node disconnection gracefully
#[tokio::test]
async fn test_bridge_node_disconnect() -> Result<()> {
    let bridge_manager = Arc::new(BridgeManager::new());

    // Register a fake node
    let (tx, _rx) = mpsc::channel::<Message>(10);
    bridge_manager
        .register_node("fake-node".to_string(), tx)
        .await;

    // Try to create a bridge to a non-existent service
    let service = Service {
        id: "nonexistent".to_string(),
        svc_type: "tcp".to_string(),
        addr: "127.0.0.1".to_string(),
        port: 9999,
        path: String::new(),
        auth: None,
    };

    // This should succeed in creating the bridge (OPEN_BRIDGE sent)
    // but the actual TCP connection will fail on the node side
    let result = bridge_manager.create_bridge("fake-node", &service).await;
    assert!(result.is_ok(), "Bridge creation should succeed");

    // Unregister node
    bridge_manager.unregister_node("fake-node").await;

    // Try to create another bridge after node disconnect
    let result = bridge_manager.create_bridge("fake-node", &service).await;
    assert!(
        result.is_err(),
        "Bridge creation should fail after node disconnect"
    );

    Ok(())
}

// Helper function to handle node connections (same as in relay main)
async fn handle_node_connection(
    socket: TcpStream,
    webrtc_bridge: Arc<WebRtcBridge>,
    bridge_manager: Arc<BridgeManager>,
) {
    use orb_node::ProtocolHandler;
    let protocol = ProtocolHandler::new(socket);
    let mut node_id = String::new();

    let (read_half, write_half) = protocol.into_split();
    let mut read_half = read_half;
    let mut write_half = write_half;

    let (tx, mut rx) = mpsc::channel::<Message>(100);

    tokio::spawn(async move {
        use tokio::io::AsyncWriteExt;
        println!("[Relay] Sender task started");
        while let Some(msg) = rx.recv().await {
            println!(
                "[Relay] Sender task received message from channel: {:?}",
                msg
            );
            println!("[Relay] Encoding and sending to node...");
            match msg.encode() {
                Ok(encoded) => {
                    let len = encoded.len() as u32;
                    println!("[Relay] Writing length prefix: {} bytes", len);
                    if write_half.write_all(&len.to_be_bytes()).await.is_err() {
                        eprintln!("[Relay] Failed to write length prefix");
                        break;
                    }
                    println!("[Relay] Writing message payload");
                    if write_half.write_all(&encoded).await.is_err() {
                        eprintln!("[Relay] Failed to write message");
                        break;
                    }
                    if write_half.flush().await.is_err() {
                        eprintln!("[Relay] Failed to flush");
                        break;
                    }
                    println!("[Relay] Message written and flushed successfully");
                }
                Err(e) => {
                    eprintln!("[Relay] Failed to encode message: {}", e);
                    break;
                }
            }
        }
        println!("[Relay] Sender task ended");
    });

    println!("[Relay] Handler entering message loop...");
    loop {
        use tokio::io::AsyncReadExt;
        println!("[Relay] Waiting to read message from node...");
        let msg_result = async {
            let mut len_buf = [0u8; 4];
            match read_half.read_exact(&mut len_buf).await {
                Ok(0) => return Ok(None),
                Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(None),
                Err(e) => return Err(anyhow::anyhow!("Failed to read: {}", e)),
                Ok(_) => {}
            }

            let len = u32::from_be_bytes(len_buf) as usize;
            println!("[Relay] Read message length from node: {} bytes", len);
            if len > 1024 * 1024 {
                return Err(anyhow::anyhow!("Message too large"));
            }

            let mut buf = vec![0u8; len];
            read_half.read_exact(&mut buf).await?;

            let msg = Message::decode(&buf)?;
            println!("[Relay] Decoded message from node: {:?}", msg);
            Ok(Some(msg))
        }
        .await;

        match msg_result {
            Ok(Some(msg)) => {
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
                        bridge_manager
                            .register_node(node_id.clone(), tx.clone())
                            .await;
                    }
                    Message::Control {
                        control: ControlMessage::Announce { services },
                        ..
                    } => {
                        let _ = webrtc_bridge.register_services(&node_id, services).await;
                    }
                    Message::Data {
                        data: DataMessage { bridge_id, payload },
                        ..
                    } => {
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
                if !node_id.is_empty() {
                    webrtc_bridge.unregister_node(&node_id).await;
                    bridge_manager.unregister_node(&node_id).await;
                }
                break;
            }
            Err(_) => {
                if !node_id.is_empty() {
                    webrtc_bridge.unregister_node(&node_id).await;
                    bridge_manager.unregister_node(&node_id).await;
                }
                break;
            }
        }
    }
}
