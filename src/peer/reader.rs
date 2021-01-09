use byteorder::{BigEndian, ReadBytesExt};
use futures::ready;
use tokio::{
    io::{AsyncRead, AsyncWrite, ReadBuf},
    net::TcpStream,
};

use std::{
    io::Cursor,
    pin::Pin,
    task::{Context, Poll},
};

use crate::{errors::TorrentError, supervisors::torrent::Result};

pub trait TryWrite {
    fn try_write(&self, _: &[u8]) -> std::io::Result<usize>;
    fn poll_writable(&self, cx: &mut Context<'_>) -> std::task::Poll<std::io::Result<()>>;
}

impl TryWrite for TcpStream {
    fn try_write(&self, buffer: &[u8]) -> std::io::Result<usize> {
        tokio::net::TcpStream::try_write(self, buffer)
    }

    fn poll_writable(&self, cx: &mut Context<'_>) -> std::task::Poll<std::io::Result<()>> {
        tokio::net::TcpStream::poll_write_ready(self, cx)
    }
}

pub trait AsyncReadWrite: AsyncRead + AsyncWrite + TryWrite + Send + Sync {}
impl<T: AsyncRead + AsyncWrite + TryWrite + Send + Sync> AsyncReadWrite for T {}

pub struct PeerReadBuffer {
    reader: Pin<Box<dyn AsyncReadWrite>>,
    buffer: Box<[u8]>,
    pos: usize,
    msg_len: usize,
    pre_data: usize,
}

impl PeerReadBuffer {
    pub fn new<T>(stream: T, piece_length: usize) -> PeerReadBuffer
    where
        T: AsyncReadWrite + 'static,
    {
        PeerReadBuffer {
            reader: Box::pin(stream),
            buffer: vec![0; piece_length].into_boxed_slice(),
            pos: 0,
            msg_len: 0,
            pre_data: 0,
        }
    }

    pub fn as_writer(&mut self) -> Pin<&mut dyn AsyncReadWrite> {
        self.reader.as_mut()
    }

    pub fn buffer(&self) -> &[u8] {
        assert_ne!(self.msg_len, 0);
        &self.buffer[self.pre_data..self.msg_len]
    }

    pub fn consume(&mut self) {
        let pos = self.pos;
        let msg_len = self.msg_len;
        self.buffer.copy_within(msg_len..pos, 0);
        self.pos -= msg_len;
        self.msg_len = 0;
        self.pre_data = 0;
    }

    fn read_at_least(&mut self, n: usize, cx: &mut Context<'_>) -> Poll<Result<()>> {
        use tokio::io::ErrorKind::UnexpectedEof;

        while self.pos < n {
            let mut buf = ReadBuf::new(&mut self.buffer[self.pos..]);
            ready!(self.reader.as_mut().poll_read(cx, &mut buf))?;
            let filled = buf.filled().len();
            if filled == 0 {
                return Poll::Ready(Err(TorrentError::IO(UnexpectedEof.into())));
            }
            self.pos += filled;
        }

        Poll::Ready(Ok(()))
    }

    fn poll_handshake(&mut self, cx: &mut Context<'_>) -> Poll<Result<()>> {
        ready!(self.read_at_least(1, cx))?;

        let length = self.buffer[0] as usize;

        ready!(self.read_at_least(length + 48, cx))?;

        self.pre_data = 1;
        self.msg_len = length + 48 + 1;
        Poll::Ready(Ok(()))
    }

    pub fn poll_next_message(&mut self, cx: &mut Context<'_>) -> Poll<Result<()>> {
        ready!(self.read_at_least(4, cx))?;

        let length = {
            let mut cursor = Cursor::new(&self.buffer);
            cursor.read_u32::<BigEndian>().unwrap() as usize
        };

        assert!(length + 4 < self.buffer.len());

        ready!(self.read_at_least(length + 4, cx))?;

        self.pre_data = 4;
        self.msg_len = 4 + length;
        Poll::Ready(Ok(()))
    }

    pub async fn read_message(&mut self) -> Result<()> {
        futures::future::poll_fn(|cx| self.poll_next_message(cx)).await
    }

    pub async fn read_handshake(&mut self) -> Result<()> {
        futures::future::poll_fn(|cx| self.poll_handshake(cx)).await
    }
}
