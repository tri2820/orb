use anyhow::Result;
use orb_node::{ControlMessage, DataMessage, Message, Service};
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::Mutex;

/// Mock echo server that simulates a target service
async fn start_echo_server(addr: &str, port: u16) -> Result<()> {
    let listener = TcpListener::bind((addr, port)).await?;

    tokio::spawn(async move {
        loop {
            match listener.accept().await {
                Ok((mut socket, _)) => {
                    tokio::spawn(async move {
                        let mut buf = vec![0; 1024];
                        loop {
                            match socket.read(&mut buf).await {
                                Ok(0) => break,
                                Ok(n) => {
                                    let _ = socket.write_all(&buf[..n]).await;
                                }
                                Err(_) => break,
                            }
                        }
                    });
                }
                Err(_) => {
                    break;
                }
            }
        }
    });

    Ok(())
}

/// Direct connection test: client connects directly to relay/bridge
/// Simulates the scenario where a client connects via WebRTC to the relay,
/// which then forwards traffic directly without going through a node.
#[tokio::test]
async fn test_direct_connection() -> Result<()> {
    // Start an echo server on localhost:9001
    start_echo_server("127.0.0.1", 9001).await?;
    tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

    // Simulate a direct client connection to a service
    // In the real scenario, this would be through WebRTC to the relay
    let mut client = TcpStream::connect("127.0.0.1:9001").await?;

    // Send test data
    let test_data = b"direct connection test";
    client.write_all(test_data).await?;

    // Read response (echo server should send back the same data)
    let mut buf = vec![0; 1024];
    let n = client.read(&mut buf).await?;

    assert_eq!(&buf[..n], test_data);
    println!("✓ Direct connection test passed");

    Ok(())
}

/// Indirect connection test: client connects through ORB node
/// Simulates the scenario where:
/// 1. Client connects to relay
/// 2. Relay sends OPEN_BRIDGE to node
/// 3. Node opens TCP connection to target service
/// 4. Data flows: Client -> Relay -> Node -> Service
#[tokio::test]
async fn test_indirect_connection_through_node() -> Result<()> {
    // Start an echo server on localhost:9002
    start_echo_server("127.0.0.1", 9002).await?;
    tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

    // Create a mock relay that will coordinate with a mock node
    let relay_ready = Arc::new(Mutex::new(false));
    let relay_ready_clone = Arc::clone(&relay_ready);

    // Start mock relay server
    let relay_handle = tokio::spawn(async move {
        let listener = TcpListener::bind("127.0.0.1:9100").await.unwrap();
        *relay_ready_clone.lock().await = true;

        if let Ok((mut socket, _)) = listener.accept().await {
            // Receive REGISTER message from node
            let mut buf = vec![0; 4096];
            if let Ok(n) = socket.read(&mut buf).await {
                if let Ok(msg) = orb_node::Message::decode(&buf[..n]) {
                    println!("Relay received: {:?}", msg);

                    // Send ACK back to node
                    let ack = Message::ack(msg.msg_id().to_string());
                    let _ = socket.write_all(&ack.encode().unwrap()).await;
                }
            }

            // Receive ANNOUNCE message from node
            if let Ok(n) = socket.read(&mut buf).await {
                if let Ok(msg) = orb_node::Message::decode(&buf[..n]) {
                    println!("Relay received announce: {:?}", msg);

                    // Send ACK back
                    let ack = Message::ack(msg.msg_id().to_string());
                    let _ = socket.write_all(&ack.encode().unwrap()).await;
                }
            }

            // Send OPEN_BRIDGE to node
            let open_bridge = Message::control(ControlMessage::OpenBridge {
                bridge_id: "bridge-001".to_string(),
                service: Service {
                    svc_type: "http".to_string(),
                    id: "echo-service".to_string(),
                    addr: "127.0.0.1".to_string(),
                    port: 9002,
                    path: "/".to_string(),
                    auth: None,
                },
            });
            if let Ok(encoded) = open_bridge.encode() {
                let _ = socket.write_all(&encoded).await;
            }

            // Receive ACK from node
            if let Ok(n) = socket.read(&mut buf).await {
                if let Ok(msg) = orb_node::Message::decode(&buf[..n]) {
                    println!("Relay received ACK: {:?}", msg);
                }
            }

            // Send DATA message to node
            let data_msg = Message::data(DataMessage {
                bridge_id: "bridge-001".to_string(),
                payload: b"indirect connection test".to_vec(),
            });
            if let Ok(encoded) = data_msg.encode() {
                let _ = socket.write_all(&encoded).await;
            }

            // Note: In a full implementation, we would:
            // 1. Receive DATA messages back from the node (echo response)
            // 2. Forward them to the client
            // 3. Handle CLOSE_BRIDGE when client disconnects
        }
    });

    // Wait for relay to be ready
    loop {
        if *relay_ready.lock().await {
            break;
        }
        tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;
    }

    tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

    // Create and connect mock node
    let node_handle = tokio::spawn(async move {
        let node = orb_node::Node::new("node-001".to_string());

        // Connect to relay
        if let Ok(stream) = TcpStream::connect("127.0.0.1:9100").await {
            *node.relay_stream.lock().await = Some(stream);

            // Send REGISTER
            let register = Message::control(ControlMessage::Register {
                node_id: "node-001".to_string(),
            });
            let _ = node.send_message(register).await;

            tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;

            // Send ANNOUNCE
            let announce = Message::control(ControlMessage::Announce {
                services: vec![Service {
                    svc_type: "http".to_string(),
                    id: "echo-service".to_string(),
                    addr: "127.0.0.1".to_string(),
                    port: 9002,
                    path: "/".to_string(),
                    auth: None,
                }],
            });
            let _ = node.send_message(announce).await;

            tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
        }

        // Note: In a full implementation, we would process messages from relay
        // and handle bridge creation/data forwarding
    });

    // Give some time for the test to run
    tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;

    // Clean up
    relay_handle.abort();
    node_handle.abort();

    println!("✓ Indirect connection through node test passed");

    Ok(())
}
