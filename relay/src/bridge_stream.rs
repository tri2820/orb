use orb_node::{DataMessage, Message};
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::sync::{mpsc, Mutex};

/// A pseudo-stream that wraps a bridge connection to provide AsyncRead/AsyncWrite interface.
/// This allows protocol clients (like RTSP) to treat a bridge connection as if it were a direct TCP stream.
pub struct BridgeStream {
    bridge_id: String,
    /// Channel to send outgoing data through the bridge
    tx: mpsc::Sender<Message>,
    /// Buffer for incoming data from the bridge
    read_buffer: Arc<Mutex<Vec<u8>>>,
    /// Channel to receive incoming data
    rx: Arc<Mutex<mpsc::Receiver<Vec<u8>>>>,
}

impl BridgeStream {
    /// Create a new BridgeStream
    ///
    /// # Arguments
    /// * `bridge_id` - The unique identifier for this bridge
    /// * `tx` - Channel to send messages to the node through the relay
    /// * `rx` - Channel to receive data from the node
    pub fn new(bridge_id: String, tx: mpsc::Sender<Message>, rx: mpsc::Receiver<Vec<u8>>) -> Self {
        Self {
            bridge_id,
            tx,
            read_buffer: Arc::new(Mutex::new(Vec::new())),
            rx: Arc::new(Mutex::new(rx)),
        }
    }

    /// Get the bridge ID
    pub fn bridge_id(&self) -> &str {
        &self.bridge_id
    }
}

impl AsyncRead for BridgeStream {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut tokio::io::ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        let this = self.get_mut();

        // Try to lock the read buffer
        let mut read_buffer = match this.read_buffer.try_lock() {
            Ok(guard) => guard,
            Err(_) => return Poll::Pending,
        };

        // If we have buffered data, read from it first
        if !read_buffer.is_empty() {
            let to_read = std::cmp::min(buf.remaining(), read_buffer.len());
            buf.put_slice(&read_buffer[..to_read]);
            read_buffer.drain(..to_read);
            return Poll::Ready(Ok(()));
        }

        // Try to receive new data from the channel
        let mut rx = match this.rx.try_lock() {
            Ok(guard) => guard,
            Err(_) => return Poll::Pending,
        };

        match rx.poll_recv(cx) {
            Poll::Ready(Some(data)) => {
                let to_read = std::cmp::min(buf.remaining(), data.len());
                buf.put_slice(&data[..to_read]);

                // Buffer any remaining data
                if to_read < data.len() {
                    read_buffer.extend_from_slice(&data[to_read..]);
                }

                Poll::Ready(Ok(()))
            }
            Poll::Ready(None) => {
                // Channel closed - EOF
                Poll::Ready(Ok(()))
            }
            Poll::Pending => Poll::Pending,
        }
    }
}

impl AsyncWrite for BridgeStream {
    fn poll_write(
        self: Pin<&mut Self>,
        _cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<Result<usize, std::io::Error>> {
        let this = self.get_mut();

        // Create a DATA message with the payload
        let msg = Message::Data {
            msg_id: uuid::Uuid::new_v4().to_string(),
            data: DataMessage {
                bridge_id: this.bridge_id.clone(),
                payload: buf.to_vec(),
            },
        };

        // Try to send the message (non-blocking)
        match this.tx.try_send(msg) {
            Ok(()) => {
                let len = buf.len();
                Poll::Ready(Ok(len))
            }
            Err(mpsc::error::TrySendError::Full(_)) => {
                // Channel is full, return pending
                Poll::Pending
            }
            Err(mpsc::error::TrySendError::Closed(_)) => Poll::Ready(Err(std::io::Error::new(
                std::io::ErrorKind::BrokenPipe,
                "Channel closed",
            ))),
        }
    }

    fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Result<(), std::io::Error>> {
        // No buffering on our side, so flush is a no-op
        Poll::Ready(Ok(()))
    }

    fn poll_shutdown(
        self: Pin<&mut Self>,
        _cx: &mut Context<'_>,
    ) -> Poll<Result<(), std::io::Error>> {
        // Send CLOSE_BRIDGE message would go here in a full implementation
        // For now, just return ready
        Poll::Ready(Ok(()))
    }
}
