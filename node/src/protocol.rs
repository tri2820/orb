use crate::message::{Message};
use anyhow::{anyhow, Result};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

/// Protocol handler for framing and serialization
pub struct ProtocolHandler {
    stream: TcpStream,
}

impl ProtocolHandler {
    pub fn new(stream: TcpStream) -> Self {
        Self { stream }
    }

    /// Send a message - handles serialization and framing
    pub async fn send(&mut self, msg: &Message) -> Result<()> {
        let encoded = msg.encode()?;
        // Write length prefix (4 bytes, big-endian)
        let len = encoded.len() as u32;
        self.stream.write_all(&len.to_be_bytes()).await?;
        // Write message
        self.stream.write_all(&encoded).await?;
        Ok(())
    }

    /// Receive a message - handles framing and deserialization
    pub async fn recv(&mut self) -> Result<Option<Message>> {
        let mut len_buf = [0u8; 4];
        match self.stream.read_exact(&mut len_buf).await {
            Ok(0) => return Ok(None), // Connection closed
            Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(None),
            Err(e) => return Err(anyhow!("Failed to read message length: {}", e)),
            Ok(_) => {}
        }

        let len = u32::from_be_bytes(len_buf) as usize;
        if len > 1024 * 1024 {
            // Sanity check: max 1MB messages
            return Err(anyhow!("Message too large: {} bytes", len));
        }

        let mut buf = vec![0u8; len];
        self.stream.read_exact(&mut buf).await?;

        let msg = Message::decode(&buf)?;
        Ok(Some(msg))
    }

    /// Get mutable reference to underlying stream (for advanced use)
    pub fn stream_mut(&mut self) -> &mut TcpStream {
        &mut self.stream
    }

    /// Split into reader and writer
    pub fn into_split(self) -> (tokio::io::BufReader<tokio::net::tcp::OwnedReadHalf>, tokio::io::BufWriter<tokio::net::tcp::OwnedWriteHalf>) {
        let (read_half, write_half) = self.stream.into_split();
        (
            tokio::io::BufReader::new(read_half),
            tokio::io::BufWriter::new(write_half),
        )
    }
}
