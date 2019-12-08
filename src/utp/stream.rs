
use async_std::sync::{Arc, channel, Sender, Receiver};
use async_std::task;
use async_std::net::{SocketAddr, UdpSocket};

use super::{UtpError, Result};

#[derive(Debug)]
struct State {
    state: State,
    ack_number: SequenceNumber,
    seq_number: SequenceNumber,
}

#[derive(Debug)]
struct UdpManager {
    socket: UdpSocket
}

#[derive(Debug)]
struct UdpReader {
    socket: UdpSocket
}

#[derive(Debug)]
struct ReaderCommand {
    length: usize
}

#[derive(Debug)]
struct ReaderResult {
    data: Vec<u8>
}

#[derive(Debug)]
struct UdpWriter {
    socket: Arc<UdpSocket>,
    command: Receiver<WriterCommand>,
    result: Sender<WriterResult>,
}

#[derive(Debug)]
enum WriterCommand {
    WriteData {
        data: Vec<u8>,
    },
    ResendPacket {

    }
}

#[derive(Debug)]
struct WriterResult {
    data: Vec<u8>
}

impl UdpWriter {
    pub fn new(
        socket: Arc<UdpSocket>,
        command: Receiver<WriterCommand>,
        result: Sender<WriterResult>,
    ) -> UdpWriter {
        UdpWriter { socket, command, result }
    }

    pub async fn start(mut self) {
        while let Some(cmd) = self.command.recv().await {
            match cmd {
                WriterCommand::WriteData { ref data } => {
                    self.send(data).await.unwrap()
                },
                WriterCommand::ResendPacket { } => {}
            }
        }
    }

    async fn send(&mut self, data: &[u8]) -> Result<()> {
        Ok(())
    }
}

#[derive(Debug)]
struct UdpStream {
    reader_command: Sender<ReaderCommand>,
    reader_result: Receiver<ReaderResult>,
    writer_command: Sender<WriterCommand>,
    writer_result: Receiver<WriterResult>,
}

impl UdpStream {
    pub async fn connect(addr: SocketAddr) -> Result<()> {
        let local: SocketAddr = if addr.is_ipv4() {
            "127.0.0.1:0".parse().unwrap()
        } else {
            "::1:0".parse().unwrap()
        };

        let udp = UdpSocket::bind(local).await?;
        udp.connect(addr).await?;

        let mut buffer = [0; 1500];
        let mut len = None;

        let mut header = Packet::syn();
        header.set_connection_id(self.recv_id);
        header.set_seq_number(self.seq_number);
        header.set_window_size(1_048_576);
        self.seq_number += 1;

        for _ in 0..3 {
            header.update_timestamp();
            println!("SENDING {:#?}", header);

            udp.send(header.as_bytes()).await?;

            match udp.recv_from_timeout(&mut buffer, Duration::from_secs(1)).await {
                Ok((n, addr)) => {
                    len = Some(n);
                    // self.remote = Some(addr);
                    let state = State::SynSent;
                    break;
                }
                Err(ref e) if e.kind() == ErrorKind::WouldBlock || e.kind() == ErrorKind::TimedOut => {
                    continue;
                }
                Err(e) => {
                    return Err(e.into());
                }
            }
        };

        if let Some(len) = len {
            println!("CONNECTED", );
            let packet = PacketRef::ref_from_buffer(&buffer[..len])?;
            // self.dispatch(packet).await?;


            let udp_clone = udp.try_clone()?;
            let udp_clone2 = udp.try_clone()?;

            task::spawn(async move {

            });

            return Ok(());
        }

        Err(Error::new(ErrorKind::TimedOut, "connect timed out").into())


        // Ok(())
    }


    // /// Addr must match the ip familly of the bind address (ipv4 / ipv6)
    // pub async fn connect(&mut self, addr: SocketAddr) -> Result<()> {
    //     self.udp.connect(addr).await?;

    //     let mut buffer = [0; 1500];
    //     let mut len = None;

    //     let mut header = Packet::syn();
    //     header.set_connection_id(self.recv_id);
    //     header.set_seq_number(self.seq_number);
    //     header.set_window_size(1_048_576);
    //     self.seq_number += 1;

    //     for _ in 0..3 {
    //         header.update_timestamp();
    //         println!("SENDING {:#?}", header);

    //         self.udp.send(header.as_bytes()).await?;

    //         match self.udp.recv_from_timeout(&mut buffer, Duration::from_secs(1)).await {
    //             Ok((n, addr)) => {
    //                 len = Some(n);
    //                 self.remote = Some(addr);
    //                 self.state = State::SynSent;
    //                 break;
    //             }
    //             Err(ref e) if e.kind() == ErrorKind::WouldBlock || e.kind() == ErrorKind::TimedOut => {
    //                 continue;
    //             }
    //             Err(e) => {
    //                 return Err(e.into());
    //             }
    //         }
    //     };

    //     if let Some(len) = len {
    //         println!("CONNECTED", );
    //         let packet = PacketRef::ref_from_buffer(&buffer[..len])?;
    //         self.dispatch(packet).await?;
    //         return Ok(());
    //     }

    //     Err(Error::new(ErrorKind::TimedOut, "connect timed out").into())
    // }

    pub fn read(&self) {

    }

    pub fn write(&self) {

    }
}
