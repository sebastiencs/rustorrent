use std::{
    convert::TryFrom,
    io::Result,
    task::{Context, Poll},
};

use crate::peer::peer::PeerExternId;

use super::{
    message::MessagePeer,
    reader::{AsyncReadWrite, PeerReadBuffer},
    writer::BufferWriter,
};

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
            match writer.try_write(self.buffer_writer.as_ref()) {
                Ok(nbytes) => {
                    self.buffer_writer.consume(nbytes);
                }
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    return Ok(());
                }
                e => return e.map(|_| ()),
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

    pub async fn wait_on_socket(&mut self) -> Result<()> {
        enum State {
            Write(Result<()>),
            Read(Result<()>),
        }

        loop {
            if self.buffer_writer.is_empty() {
                return self.reader.read_message().await;
            }

            let fun = |cx: &mut Context<'_>| {
                if let Poll::Ready(v) = self.reader.poll_next_message(cx) {
                    return Poll::Ready(State::Read(v));
                };

                self.reader.as_writer().poll_writable(cx).map(State::Write)
            };

            match futures::future::poll_fn(fun).await {
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

    pub fn get_message(&self) -> crate::supervisors::torrent::Result<MessagePeer> {
        MessagePeer::try_from(self.reader.buffer())
    }

    pub fn consume_read(&mut self) {
        self.reader.consume();
    }
}
