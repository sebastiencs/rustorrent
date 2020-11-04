
use std::time::Duration;
use std::net::SocketAddr;

use tokio::net::UdpSocket;
use async_trait::async_trait;

#[async_trait]
pub trait WithTimeout {
    async fn recv_timeout(&self, buf: &mut [u8], timeout: Duration) -> tokio::io::Result<usize>;
    async fn recv_from_timeout(&self, buf: &mut [u8], timeout: Duration) -> tokio::io::Result<(usize, SocketAddr)>;
    async fn send_timeout(&self, buf: &[u8], timeout: Duration) -> tokio::io::Result<usize>;
}

#[async_trait]
impl WithTimeout for UdpSocket {
    async fn recv_timeout(&self, buf: &mut [u8], timeout: Duration) -> tokio::io::Result<usize> {
        tokio::time::timeout(timeout, async move { self.recv(buf).await }).await?
    }

    async fn recv_from_timeout(&self, buf: &mut [u8], timeout: Duration) -> tokio::io::Result<(usize, SocketAddr)> {
        tokio::time::timeout(timeout, async move { self.recv_from(buf).await }).await?
    }

    async fn send_timeout(&self, buf: &[u8], timeout: Duration) -> tokio::io::Result<usize> {
        tokio::time::timeout(timeout, async move { self.send(buf).await }).await?
    }
}
