
use std::time::Duration;

use async_std::net::{UdpSocket, SocketAddr};
use async_std::io;
use async_trait::async_trait;

#[async_trait]
pub trait WithTimeout {
    async fn recv_timeout(&self, buf: &mut [u8], timeout: Duration) -> async_std::io::Result<usize>;
    async fn recv_from_timeout(&self, buf: &mut [u8], timeout: Duration) -> async_std::io::Result<(usize, SocketAddr)>;
    async fn send_timeout(&self, buf: &[u8], timeout: Duration) -> async_std::io::Result<usize>;
}

#[async_trait]
impl WithTimeout for UdpSocket {
    async fn recv_timeout(&self, buf: &mut [u8], timeout: Duration) -> async_std::io::Result<usize> {
        io::timeout(timeout, async move { self.recv(buf).await }).await
    }

    async fn recv_from_timeout(&self, buf: &mut [u8], timeout: Duration) -> async_std::io::Result<(usize, SocketAddr)> {
        io::timeout(timeout, async move { self.recv_from(buf).await }).await
    }

    async fn send_timeout(&self, buf: &[u8], timeout: Duration) -> async_std::io::Result<usize> {
        io::timeout(timeout, async move { self.send(buf).await }).await
    }
}
