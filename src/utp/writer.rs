use std::net::SocketAddr;

use async_std::net::UdpSocket;
use async_std::sync::{Arc, Receiver};
use shared_arena::{SharedArena, ArenaBox};
use futures::{pin_mut, FutureExt, future};
use futures::task::{Context, Poll};

use super::SequenceNumber;
use super::{Packet, PacketType, UtpError, Result, UDP_IPV4_MTU, HEADER_SIZE, UDP_IPV6_MTU};
use super::stream::State;

#[derive(Debug)]
pub(super) struct UtpWriter {
    socket: Arc<UdpSocket>,
    addr: SocketAddr,
    user_command: Receiver<WriterUserCommand>,
    command: Receiver<WriterCommand>,
    state: Arc<State>,
    packet_arena: Arc<SharedArena<Packet>>
}

impl Drop for UtpWriter {
    fn drop(&mut self) {
        println!("UtpWriter Dropped", );
    }
}

#[derive(Debug)]
pub(super) enum WriterCommand {
    WriteData {
        data: Vec<u8>,
    },
    SendPacket {
        packet_type: PacketType
    },
    ResendPacket {
        only_lost: bool
    },
    Acks,
    // Acks {
    //     number: SequenceNumber
    // },
}

pub(super) struct WriterUserCommand {
    pub(super) data: Vec<u8>
}

impl UtpWriter {
    pub fn new(
        socket: Arc<UdpSocket>,
        addr: SocketAddr,
        user_command: Receiver<WriterUserCommand>,
        command: Receiver<WriterCommand>,
        state: Arc<State>,
        packet_arena: Arc<SharedArena<Packet>>
    ) -> UtpWriter {
        UtpWriter { socket, addr, user_command, command, state, packet_arena }
    }

    async fn poll(&self) -> Result<WriterCommand> {
        let user_cmd = self.user_command.recv();
        let cmd = self.command.recv();
        pin_mut!(user_cmd); // Pin on the stack
        pin_mut!(cmd); // Pin on the stack

        let fun = |cx: &mut Context<'_>| {
            match FutureExt::poll_unpin(&mut cmd, cx) {
                v @ Poll::Ready(_) => return v,
                _ => {}
            }

            match FutureExt::poll_unpin(&mut user_cmd, cx).map(|cmd| {
                cmd.map(|c| WriterCommand::WriteData { data: c.data })
            }) {
                v @ Poll::Ready(_) => v,
                _ => Poll::Pending
            }
        };

        future::poll_fn(fun).await.map_err(UtpError::RecvError)
    }

    pub async fn start(mut self) {
        while let Ok(cmd) = self.poll().await {
            self.handle_cmd(cmd).await.unwrap();
        }
    }

    async fn handle_cmd(&mut self, cmd: WriterCommand) -> Result<()> {
        use WriterCommand::*;

        match cmd {
            WriteData { ref data } => {
                // loop {
                    self.send(data).await.unwrap();
                    println!("== SEND DATA {:#?}", self.packet_arena);
                    //self.packet_arena.resize(1600);
                // }
            },
            SendPacket { packet_type } => {
                if packet_type == PacketType::Syn {
                    self.send_syn().await.unwrap();
                } else {
                    let packet = self.packet_arena.alloc(Packet::new_type(packet_type));
                    self.send_packet(packet).await.unwrap();
                }
            }
            ResendPacket { only_lost } => {
                self.resend_packets(only_lost).await.unwrap();
            }
            // Acks { number }  => {
            Acks => {
                return Ok(());
            }
        }
        Ok(())
    }

    async fn handle_manager_cmd(&mut self, cmd: WriterCommand) -> Result<()> {
        use WriterCommand::*;

        match cmd {
            SendPacket { packet_type } => {
                if packet_type == PacketType::Syn {
                    self.send_syn().await.unwrap();
                } else {
                    let packet = self.packet_arena.alloc(Packet::new_type(packet_type));
                    self.send_packet(packet).await.unwrap();
                }
            }
            ResendPacket { only_lost } => {
                self.resend_packets(only_lost).await.unwrap();
            }
            // Acks { number } => {
            Acks => {
                return Ok(());
            }
            WriteData { .. } => {
                unreachable!();
            },
        }
        Ok(())
    }

    async fn resend_packets(&mut self, only_lost: bool) -> Result<()> {
        let mut inflight_packets = self.state.inflight_packets.write().await;

        // println!("RESENDING ONLY_LOST={:?} INFLIGHT_LEN={:?} CWND={:?}", only_lost, inflight_packets.len(), self.state.cwnd());

        let mut resent = 0;

        for packet in inflight_packets.values_mut() {
            if !only_lost || packet.lost {
                packet.set_ack_number(self.state.ack_number());
                packet.update_timestamp();

                // println!("== RESENDING {:?}", &**packet);
//                println!("== RESENDING {:?} CWND={:?} POS {:?} {:?}", start, self.cwnd, start, &**packet);

                resent += 1;
                self.socket.send_to(packet.as_bytes(), self.addr).await?;

                packet.resent = true;
                packet.lost = false;
            }
        }

        println!("RESENT: {:?}", resent);
        Ok(())
    }

    async fn send(&mut self, data: &[u8]) -> Result<()> {
        let packet_size = self.packet_size();

        let packets = data.chunks(packet_size);

        for packet in packets {
            let packet = self.packet_arena.alloc_with(|packet_uninit| {
                Packet::new_in_place(packet_uninit, packet)
            });

            // let data = arena.alloc_with(|uninit| {
            //     Packet::new_with(uninit, source)
            // })

            self.ensure_window_is_large_enough(packet.size()).await?;
            self.send_packet(packet).await?;
        }

        eprintln!("DOOOONE", );

        Ok(())
    }

    fn packet_size(&self) -> usize {
        let is_ipv4 = self.addr.is_ipv4();

        // TODO: Change this when MTU discovery is implemented
        if is_ipv4 {
            UDP_IPV4_MTU - HEADER_SIZE
        } else {
            UDP_IPV6_MTU - HEADER_SIZE
        }
    }

    async fn ensure_window_is_large_enough(&mut self, packet_size: usize) -> Result<()> {
        while packet_size + self.state.inflight_size() > self.state.cwnd() as usize {
            // println!("BLOCKED BY CWND {:?} {:?} {:?}", packet_size, self.state.inflight_size(), self.state.cwnd());

            // Too many packets in flight, we wait for acked or lost packets
            match self.command.recv().await {
                Ok(cmd) => self.handle_manager_cmd(cmd).await?,
                _ => return Err(UtpError::MustClose)
            }
        }
        Ok(())
    }

    async fn send_packet(&mut self, mut packet: ArenaBox<Packet>) -> Result<()> {
        let ack_number = self.state.ack_number();
        let seq_number = self.state.increment_seq();
        let send_id = self.state.send_id();

        packet.set_ack_number(ack_number);
        packet.set_packet_seq_number(seq_number);
        packet.set_connection_id(send_id);
        packet.set_window_size(1_048_576);

        // println!("SENDING NEW PACKET ! {:?}", packet);

        packet.update_timestamp();

        self.socket.send_to(packet.as_bytes(), self.addr).await?;

        if packet.get_type()? != PacketType::State {
            self.state.add_packet_inflight(seq_number, packet).await;
        }

        Ok(())
    }

    async fn send_syn(&mut self) -> Result<()> {
        let seq_number = self.state.increment_seq();
        let recv_id = self.state.recv_id();

        let mut packet = Packet::syn();
        packet.set_connection_id(recv_id);
        packet.set_packet_seq_number(seq_number);
        packet.set_window_size(1_048_576);

        //println!("SENDING {:#?}", packet);

        packet.update_timestamp();
        self.socket.send_to(packet.as_bytes(), self.addr).await?;

        self.state.set_utp_state(super::UtpState::SynSent);

        let packet = self.packet_arena.alloc(packet);
        self.state.add_packet_inflight(seq_number, packet).await;

        Ok(())
    }
}
