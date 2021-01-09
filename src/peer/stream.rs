use std::{
    convert::TryFrom,
    task::{Context, Poll},
};

use crate::actors::peer::PeerExternId;

use super::{
    message::MessagePeer,
    reader::{AsyncReadWrite, PeerReadBuffer},
    writer::BufferWriter,
};
use crate::supervisors::torrent::Result;

pub struct StreamBuffers {
    reader: PeerReadBuffer,
    buffer_writer: BufferWriter,
}

impl StreamBuffers {
    pub fn new<T>(stream: T, read_buffer_length: usize, write_buffer_length: usize) -> Self
    where
        T: AsyncReadWrite + 'static,
    {
        Self {
            reader: PeerReadBuffer::new(stream, read_buffer_length),
            buffer_writer: BufferWriter::new(write_buffer_length),
        }
    }

    fn write_to_socket(&mut self) -> Result<()> {
        let writer = self.reader.as_writer();

        while !self.buffer_writer.is_empty() {
            match writer.try_write(self.buffer_writer.get_buffer()) {
                Ok(nbytes) => {
                    self.buffer_writer.consume(nbytes);
                }
                Err(e) => {
                    if let std::io::ErrorKind::WouldBlock = e.kind() {
                        return Ok(());
                    }
                    return Err(e.into());
                }
            }
        }

        Ok(())
    }

    pub fn write_message<'a, M>(&mut self, msg: M) -> Result<()>
    where
        M: Into<MessagePeer<'a>>,
    {
        self.buffer_writer.write_msg(msg);
        self.write_to_socket()
    }

    pub async fn read_message(&mut self) -> Result<()> {
        enum State {
            Write(Result<()>),
            Read(Result<()>),
        }

        loop {
            if self.buffer_writer.is_empty() {
                return self.reader.read_message().await;
            }

            let mut fun = |cx: &mut Context<'_>| {
                if let std::task::Poll::Ready(v) = self.reader.poll_next_message(cx) {
                    return Poll::Ready(State::Read(v));
                };

                let writer = self.reader.as_writer();
                if let std::task::Poll::Ready(v) = writer.poll_writable(cx) {
                    return Poll::Ready(State::Write(v.map_err(|e| e.into())));
                }

                Poll::Pending
            };

            match futures::future::poll_fn(|cx| fun(cx)).await {
                State::Read(v) => return v,
                State::Write(v) => {
                    v?;
                    self.write_to_socket()?;
                }
            }
        }
    }

    pub async fn read_handshake(&mut self) -> Result<PeerExternId> {
        self.reader.read_handshake().await?;
        let buffer = self.reader.buffer();
        let length = buffer.len();

        let peer_id = PeerExternId::new(&buffer[length - 20..]);
        self.reader.consume();

        Ok(peer_id)
    }

    pub fn get_message(&self) -> Result<MessagePeer> {
        MessagePeer::try_from(self.reader.buffer())
    }

    pub fn consume_read(&mut self) {
        self.reader.consume();
    }
}
