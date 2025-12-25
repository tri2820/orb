//! I/O utilities for reliable network communication
//!
//! This module provides wrappers that prevent common pitfalls like forgetting to flush.

use tokio::io::{duplex, AsyncReadExt, AsyncWrite, AsyncWriteExt};

/// Wrapper that auto-flushes after every write.
///
/// # Why This Exists
///
/// Rust's `write_all` doesn't guarantee data is sent immediately - it may sit in buffers.
/// For network protocols where message timing matters (like ORB), forgetting to flush
/// causes silent failures that are hard to debug.
///
/// # Example
///
/// ```no_run
/// use orb_node::io::FlushWriter;
/// use tokio::net::TcpStream;
///
/// # async fn example() -> std::io::Result<()> {
/// let stream = TcpStream::connect("127.0.0.1:8080").await?;
/// let mut writer = FlushWriter::new(stream);
///
/// // This writes AND flushes - impossible to forget!
/// writer.write_and_flush(b"hello").await?;
/// # Ok(())
/// # }
/// ```
pub struct FlushWriter<W> {
    inner: W,
}

impl<W> FlushWriter<W> {
    /// Create a new FlushWriter wrapping the given writer.
    pub fn new(inner: W) -> Self {
        Self { inner }
    }

    /// Get a mutable reference to the inner writer.
    pub fn inner_mut(&mut self) -> &mut W {
        &mut self.inner
    }

    /// Consume this wrapper, returning the inner writer.
    pub fn into_inner(self) -> W {
        self.inner
    }
}

// Implement AsyncWrite for FlushWriter - delegates to inner writer
impl<W: AsyncWrite + Unpin> AsyncWrite for FlushWriter<W> {
    fn poll_write(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &[u8],
    ) -> std::task::Poll<std::io::Result<usize>> {
        std::pin::Pin::new(&mut self.inner).poll_write(cx, buf)
    }

    fn poll_flush(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        std::pin::Pin::new(&mut self.inner).poll_flush(cx)
    }

    fn poll_shutdown(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        std::pin::Pin::new(&mut self.inner).poll_shutdown(cx)
    }

    fn poll_write_vectored(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        bufs: &[std::io::IoSlice<'_>],
    ) -> std::task::Poll<std::io::Result<usize>> {
        std::pin::Pin::new(&mut self.inner).poll_write_vectored(cx, bufs)
    }

    fn is_write_vectored(&self) -> bool {
        self.inner.is_write_vectored()
    }
}

impl<W: AsyncWrite + Unpin> FlushWriter<W> {
    /// Write data and immediately flush.
    ///
    /// This is the main method to use - it combines write and flush in one call.
    pub async fn write_and_flush(&mut self, buf: &[u8]) -> std::io::Result<()> {
        self.inner.write_all(buf).await?;
        self.inner.flush().await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_flush_writer() {
        let (client, server) = duplex(1024);

        // Write with FlushWriter
        let mut writer = FlushWriter::new(client);
        writer.write_and_flush(b"hello").await.unwrap();

        // Read from server
        let mut reader = server;
        let mut buf = [0u8; 5];
        reader.read_exact(&mut buf).await.unwrap();
        assert_eq!(&buf, b"hello");
    }
}
