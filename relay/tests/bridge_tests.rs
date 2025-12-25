use anyhow::Result;
use orb_node::{Service, Node};
use orb_relay::{bridge_manager::BridgeManager, webrtc::WebRtcBridge};
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
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

    // Spawn relay node handler using the new handle_node method
    let bridge_mgr_clone = Arc::clone(&bridge_manager);
    let webrtc_clone = Arc::clone(&webrtc_bridge);
    tokio::spawn(async move {
        println!("[Relay] Waiting for node connection...");
        if let Ok((socket, _)) = relay_listener.accept().await {
            println!("[Relay] Node connected, starting handler");
            let _ = bridge_mgr_clone.handle_node(socket, webrtc_clone).await;
            println!("[Relay] Handler finished");
        }
    });

    // Create and connect node using the new run() API
    let service = Service {
        id: "echo-service".to_string(),
        svc_type: "tcp".to_string(),
        addr: echo_addr.ip().to_string(),
        port: echo_addr.port(),
        path: String::new(),
        auth: None,
    };

    // Clone service for use after spawn
    let service_for_bridge = service.clone();
    let relay_addr_str = relay_addr.ip().to_string();

    let node = Node::new("test-node".to_string());
    // Spawn node in background since run() blocks
    let node_handle = tokio::spawn(async move {
        let _ = node.run(&relay_addr_str, relay_addr.port(), vec![service]).await;
    });

    // Give the node time to connect and announce
    tokio::time::sleep(Duration::from_millis(200)).await;

    // Register services with WebRTC bridge (normally done by relay when announce is received)
    webrtc_bridge
        .register_services("test-node", vec![service_for_bridge.clone()])
        .await?;

    // Give everything time to connect
    tokio::time::sleep(Duration::from_millis(100)).await;

    // Create a bridge
    let bridge_stream = bridge_manager.create_bridge("test-node", &service_for_bridge).await?;

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

    // Clean up
    node_handle.abort();

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

    // Spawn relay node handler
    let bridge_mgr_clone = Arc::clone(&bridge_manager);
    let webrtc_clone = Arc::clone(&webrtc_bridge);
    tokio::spawn(async move {
        if let Ok((socket, _)) = relay_listener.accept().await {
            let _ = bridge_mgr_clone.handle_node(socket, webrtc_clone).await;
        }
    });

    // Create services
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

    // Clone services for use after spawn
    let service1_for_bridge = service1.clone();
    let service2_for_bridge = service2.clone();
    let relay_addr_str = relay_addr.ip().to_string();

    // Create node with two services using the new run() API
    let node = Node::new("multi-test-node".to_string());
    let node_handle = tokio::spawn(async move {
        let _ = node.run(
            &relay_addr_str,
            relay_addr.port(),
            vec![service1, service2]
        ).await;
    });

    // Give the node time to connect and announce
    tokio::time::sleep(Duration::from_millis(200)).await;

    // Register services with WebRTC bridge
    webrtc_bridge
        .register_services("multi-test-node", vec![service1_for_bridge.clone(), service2_for_bridge.clone()])
        .await?;

    tokio::time::sleep(Duration::from_millis(100)).await;

    // Create two bridges simultaneously
    let bridge1 = bridge_manager
        .create_bridge("multi-test-node", &service1_for_bridge)
        .await?;
    let bridge2 = bridge_manager
        .create_bridge("multi-test-node", &service2_for_bridge)
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

    // Clean up
    node_handle.abort();

    Ok(())
}

/// Test that bridge handles node disconnection gracefully
#[tokio::test]
async fn test_bridge_node_disconnect() -> Result<()> {
    let bridge_manager = Arc::new(BridgeManager::new());

    // Register a fake node
    let (tx, _rx) = mpsc::channel::<orb_node::Message>(10);
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
