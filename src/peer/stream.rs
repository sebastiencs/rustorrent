use std::convert::TryFrom;

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

    pub async fn write_message<'m, M>(&mut self, msg: M) -> Result<()>
    where
        M: Into<MessagePeer<'m>>,
    {
        use tokio::io::AsyncWriteExt;

        self.buffer_writer.write_msg(msg.into());

        let mut writer = self.reader.as_writer();
        writer.write_all(self.buffer_writer.as_ref()).await?;
        writer.flush().await?;

        Ok(())
    }

    pub async fn read_message(&mut self) -> Result<()> {
        self.reader.read_message().await
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
