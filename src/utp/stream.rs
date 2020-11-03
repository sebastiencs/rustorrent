
use async_std::sync::Receiver;
use async_std::sync::{Sender, RwLock};

use crate::utils::Map;
use shared_arena::ArenaBox;
use super::writer::WriterUserCommand;

use super::{
    SequenceNumber, Packet,
    ConnectionId,
};
use super::{
    INIT_CWND, MSS,
    UtpState,
};
use crate::utils::FromSlice;

use std::sync::atomic::{AtomicU8, AtomicU16, AtomicU32, Ordering};

#[derive(Debug)]
pub(super) struct State {
    pub(super) utp_state: AtomicU8,
    pub(super) recv_id: AtomicU16,
    pub(super) send_id: AtomicU16,
    pub(super) ack_number: AtomicU16,
    pub(super) seq_number: AtomicU16,
    pub(super) remote_window: AtomicU32,
    pub(super) cwnd: AtomicU32,
    pub(super) in_flight: AtomicU32,

    /// Packets sent but we didn't receive an ack for them
    pub(super) inflight_packets: RwLock<Map<SequenceNumber, ArenaBox<Packet>>>,
}

impl State {
    pub(super) async fn add_packet_inflight(&self, seq_num: SequenceNumber, packet: ArenaBox<Packet>) {
        let size = packet.size();

        {
            let mut inflight_packets = self.inflight_packets.write().await;
            inflight_packets.insert(seq_num, packet);
        }

        self.in_flight.fetch_add(size as u32, Ordering::AcqRel);
    }

    pub(super) async fn remove_packets(&self, ack_number: SequenceNumber) -> usize {
        let mut size = 0;

        {
            let mut inflight_packets = self.inflight_packets.write().await;
            inflight_packets
                .retain(|_, p| {
                    !p.is_seq_less_equal(ack_number) || (false, size += p.size()).0
                });
        }

        self.in_flight.fetch_sub(size as u32, Ordering::AcqRel);
        size
    }

    pub(super) async fn remove_packet(&self, ack_number: SequenceNumber) -> usize {
        let size = {
            let mut inflight_packets = self.inflight_packets.write().await;

            inflight_packets.remove(&ack_number)
                            .map(|p| p.size())
                            .unwrap_or(0)
        };

        self.in_flight.fetch_sub(size as u32, Ordering::AcqRel);
        size
    }

    pub(super) fn inflight_size(&self) -> usize {
        self.in_flight.load(Ordering::Acquire) as usize
    }

    pub(super) fn utp_state(&self) -> UtpState {
        self.utp_state.load(Ordering::Acquire).into()
    }
    pub(super) fn set_utp_state(&self, state: UtpState) {
        self.utp_state.store(state.into(), Ordering::Release)
    }
    pub(super) fn recv_id(&self) -> ConnectionId {
        self.recv_id.load(Ordering::Relaxed).into()
    }
    pub(super) fn set_recv_id(&self, recv_id: ConnectionId) {
        self.recv_id.store(recv_id.into(), Ordering::Release)
    }
    pub(super) fn send_id(&self) -> ConnectionId {
        self.send_id.load(Ordering::Relaxed).into()
    }
    pub(super) fn set_send_id(&self, send_id: ConnectionId) {
        self.send_id.store(send_id.into(), Ordering::Release)
    }
    pub(super) fn ack_number(&self) -> SequenceNumber {
        self.ack_number.load(Ordering::Acquire).into()
    }
    pub(super) fn set_ack_number(&self, ack_number: SequenceNumber) {
        self.ack_number.store(ack_number.into(), Ordering::Release)
    }
    pub(super) fn seq_number(&self) -> SequenceNumber {
        self.seq_number.load(Ordering::Acquire).into()
    }
    pub(super) fn set_seq_number(&self, seq_number: SequenceNumber) {
        self.seq_number.store(seq_number.into(), Ordering::Release)
    }
    /// Increment seq_number and returns its previous value
    pub(super) fn increment_seq(&self) -> SequenceNumber {
        self.seq_number.fetch_add(1, Ordering::AcqRel).into()
    }
    pub(super) fn remote_window(&self) -> u32 {
        self.remote_window.load(Ordering::Acquire)
    }
    pub(super) fn set_remote_window(&self, remote_window: u32) {
        self.remote_window.store(remote_window, Ordering::Release)
    }
    pub(super) fn cwnd(&self) -> u32 {
        self.cwnd.load(Ordering::Acquire)
    }
    pub(super) fn set_cwnd(&self, cwnd: u32) {
        self.cwnd.store(cwnd, Ordering::Release)
    }
}

impl State {
    pub(super) fn with_utp_state(utp_state: UtpState) -> State {
        State {
            utp_state: AtomicU8::new(utp_state.into()),
            ..Default::default()
        }
    }
}

impl Default for State {
    fn default() -> State {
        let (recv_id, send_id) = ConnectionId::make_ids();

        State {
            utp_state: AtomicU8::new(UtpState::None.into()),
            recv_id: AtomicU16::new(recv_id.into()),
            send_id: AtomicU16::new(send_id.into()),
            ack_number: AtomicU16::new(SequenceNumber::zero().into()),
            seq_number: AtomicU16::new(SequenceNumber::random().into()),
            remote_window: AtomicU32::new(INIT_CWND * MSS),
            cwnd: AtomicU32::new(INIT_CWND * MSS),
            in_flight: AtomicU32::new(0),
            inflight_packets: Default::default(),
        }
    }
}

pub(super) enum ReceivedData {
    Packet { },
    Done
}

#[derive(Debug)]
pub struct UtpStream {
    // reader_command: Sender<ReaderCommand>,
    pub(super) receive: Receiver<ReceivedData>,
    pub(super) writer_user_command: Sender<WriterUserCommand>,
}

impl UtpStream {
    pub async fn read(&self, _data: &mut [u8]) {
        // self.reader_command.send(ReaderCommand {
        //     length: data.len()
        // }).await;

        // self.reader_result.recv().await;
    }

    pub async fn write(&self, data: &[u8]) {
        let data = Vec::from_slice(data);
        self.writer_user_command.send(WriterUserCommand { data }).await;
    }

    pub async fn wait_for_termination(&self) {
        if let Ok(ReceivedData::Done) = self.receive.recv().await {
            println!("Done received");
        }
    }
}
