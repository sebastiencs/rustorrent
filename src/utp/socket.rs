
use async_std::net::{UdpSocket, SocketAddr};
use async_std::io::{ErrorKind, Error};
use rand::Rng;

use std::time::Duration;

use super::{ConnectionId, Result, UtpError, Packet, PacketRef, PacketType, Header};
use crate::udp_ext::WithTimeout;

#[derive(Debug, PartialEq, Eq, Copy, Clone)]
enum State {
    /// not yet connected
	None,
	/// sent a syn packet, not received any acks
	SynSent,
	/// syn-ack received and in normal operation
	/// of sending and receiving data
	Connected,
	/// fin sent, but all packets up to the fin packet
	/// have not yet been acked. We might still be waiting
	/// for a FIN from the other end
	FinSent,
	// /// ====== states beyond this point =====
	// /// === are considered closing states ===
	// /// === and will cause the socket to ====
	// /// ============ be deleted =============
	// /// the socket has been gracefully disconnected
	// /// and is waiting for the client to make a
	// /// socket call so that we can communicate this
	// /// fact and actually delete all the state, or
	// /// there is an error on this socket and we're
	// /// waiting to communicate this to the client in
	// /// a callback. The error in either case is stored
	// /// in m_error. If the socket has gracefully shut
	// /// down, the error is error::eof.
	// ErrorWait,
	// /// there are no more references to this socket
	// /// and we can delete it
	// Delete
}

pub struct UtpSocket {
    local: SocketAddr,
    remote: Option<SocketAddr>,
    udp: UdpSocket,
    recv_id: ConnectionId,
    send_id: ConnectionId,
    // seq_nr: u16,
    state: State,
    ack_number: u16,
    seq_number: u16,
}

impl UtpSocket {
    fn new(local: SocketAddr, udp: UdpSocket) -> UtpSocket {
        let (recv_id, send_id) = ConnectionId::make_ids();

        UtpSocket {
            local,
            udp,
            recv_id,
            send_id,
            remote: None,
            // seq_nr: 0,
            state: State::None,
            ack_number: 0,
            seq_number: 1,
        }
    }

    pub async fn bind(addr: SocketAddr) -> Result<UtpSocket> {
        let udp = UdpSocket::bind(addr).await?;

        Ok(Self::new(addr, udp))
    }

    /// Addr must match the ip familly of the bind address (ipv4 / ipv6)
    pub async fn connect(&mut self, addr: SocketAddr) -> Result<()> {
        if addr.is_ipv4() != self.local.is_ipv4() {
            return Err(UtpError::FamillyMismatch);
        }

        self.udp.connect(addr).await?;

        let mut buffer = [0; 1500];
        let mut len = None;

        let mut header = Header::new(PacketType::Syn);
        header.set_connection_id(self.recv_id);
        header.set_seq_number(self.seq_number);

        for _ in 0..3 {
            header.update_timestamp();

            self.udp.send(header.as_bytes()).await?;

            match self.udp.recv_from_timeout(&mut buffer, Duration::from_secs(1)).await {
                Ok((n, addr)) => {
                    len = Some(n);
                    self.remote = Some(addr);
                    self.state = State::SynSent;
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
            let packet = PacketRef::ref_from_buffer(&buffer[..len])?;
            self.dispatch(packet).await?;
            return Ok(());
        }

        Err(Error::new(ErrorKind::TimedOut, "connect timed out").into())
    }

    async fn dispatch(&mut self, packet: PacketRef<'_>) -> Result<()> {
        println!("HEADER: {:#?}", packet.header());

        match (packet.get_type()?, self.state) {
            (PacketType::Syn, State::None) => {
                self.state = State::Connected;
                // Set self.remote
                self.ack_number = packet.get_seq_number();
                self.seq_number = rand::thread_rng().gen();
            }
            (PacketType::Syn, _) => {
            }
            (PacketType::State, State::SynSent) => {
                self.state = State::Connected;
                self.ack_number = packet.get_seq_number();
                println!("CONNECTED !", );
            }
            (PacketType::State, State::Connected) => {
                // Ack received
            }
            (PacketType::State, _) => {
                // Wrong Packet
            }
            (PacketType::Data, _) => {
            }
            (PacketType::Fin, _) => {
            }
            (PacketType::Reset, _) => {
            }
        }

        Ok(())
    }
}
