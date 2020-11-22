use async_channel::{Receiver, Sender, TryRecvError};

use crate::utils::Map;
use shared_arena::ArenaBox;
//use super::writer::WriterUserCommand;

use super::{manager::UtpEvent, ConnectionId, Packet, SequenceNumber, UtpState, INIT_CWND, MSS};
use crate::utils::FromSlice;

use std::{
    cell::{Cell, RefCell},
    io::{Cursor, Write},
    sync::atomic::{AtomicU16, AtomicU32, AtomicU8, Ordering},
};

// pub struct WriterUserCommand {
//     pub(super) data: Vec<u8>
// }

// impl std::fmt::Debug for WriterUserCommand {
//     fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
//         f.debug_struct("WriterUserCommand")
//          .finish()
//     }
// }

#[derive(Debug)]
pub(super) struct State {
    pub(super) utp_state: AtomicU8,
    pub(super) recv_id: AtomicU16,
    pub(super) send_id: AtomicU16,
    //pub(super) ack_number: AtomicU16,
    pub(super) seq_number: AtomicU16,
    pub(super) remote_window: AtomicU32,
    pub(super) cwnd: AtomicU32,
    pub(super) in_flight: AtomicU32,

    /// Packets sent but we didn't receive an ack for them
    pub(super) inflight_packets: Map<SequenceNumber, ArenaBox<Packet>>,
}

impl State {
    pub(super) fn add_packet_inflight(
        &mut self,
        seq_num: SequenceNumber,
        packet: ArenaBox<Packet>,
    ) {
        let size = packet.size();

        self.inflight_packets.insert(seq_num, packet);
        // {
        //     let mut inflight_packets = self.inflight_packets.write().await;
        //     inflight_packets.insert(seq_num, packet);
        // }

        self.in_flight.fetch_add(size as u32, Ordering::AcqRel);
    }

    pub(super) fn remove_packets(&mut self, ack_number: SequenceNumber) -> usize {
        let mut size = 0;

        self.inflight_packets
            .retain(|_, p| !p.is_seq_less_equal(ack_number) || (false, size += p.size()).0);

        // {
        //     let mut inflight_packets = self.inflight_packets.write().await;
        //     inflight_packets
        //         .retain(|_, p| {
        //             !p.is_seq_less_equal(ack_number) || (false, size += p.size()).0
        //         });
        // }

        self.in_flight.fetch_sub(size as u32, Ordering::AcqRel);
        size
    }

    pub(super) fn remove_packet(&mut self, ack_number: SequenceNumber) -> usize {
        let size = {
            self.inflight_packets
                .remove(&ack_number)
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
    // pub(super) fn ack_number(&self) -> SequenceNumber {
    //     self.ack_number.load(Ordering::Acquire).into()
    // }
    // pub(super) fn set_ack_number(&self, ack_number: SequenceNumber) {
    //     self.ack_number.store(ack_number.into(), Ordering::Release)
    // }
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
            //ack_number: AtomicU16::new(SequenceNumber::zero().into()),
            seq_number: AtomicU16::new(SequenceNumber::random().into()),
            remote_window: AtomicU32::new(INIT_CWND * MSS),
            cwnd: AtomicU32::new(INIT_CWND * MSS),
            in_flight: AtomicU32::new(0),
            inflight_packets: Default::default(),
        }
    }
}

#[derive(Debug)]
pub(super) enum ReceivedData {
    Data { packet: ArenaBox<Packet> },
    FirstSequence { seq: SequenceNumber },
    Done,
}

#[derive(Debug)]
pub struct UtpStream {
    pub(super) receive: Receiver<ReceivedData>,
    pub(super) writer_user_command: Sender<UtpEvent>,
    pub(super) received_data: RefCell<Map<SequenceNumber, ArenaBox<Packet>>>,
    pub(super) last_seq: Cell<Option<SequenceNumber>>,
    pub(super) cursor: Cell<usize>,
    pub(super) done: Cell<bool>,
}

impl UtpStream {
    pub(super) fn new(
        receive: Receiver<ReceivedData>,
        writer_user_command: Sender<UtpEvent>,
    ) -> UtpStream {
        UtpStream {
            receive,
            writer_user_command,
            received_data: RefCell::new(Default::default()),
            last_seq: Cell::new(None),
            cursor: Cell::new(0),
            done: Cell::new(false),
        }
    }

    pub async fn read(&self, data: &mut [u8]) -> std::io::Result<usize> {
        use ReceivedData::*;

        let mut user_data = Cursor::new(data);
        let mut current = None;

        loop {
            'inner: loop {
                match current.take().unwrap_or_else(|| self.receive.try_recv()) {
                    Ok(Data { packet }) => {
                        assert!(self.last_seq.get().is_some());
                        let mut map = self.received_data.borrow_mut();
                        map.insert(packet.get_seq_number(), packet);
                    }
                    Err(TryRecvError::Empty) => {
                        break 'inner;
                    }
                    Ok(FirstSequence { seq }) => {
                        assert!(self.last_seq.get().is_none());
                        self.last_seq.set(Some(seq));
                    }
                    Ok(Done) => {
                        self.done.set(true);
                        break 'inner;
                    }
                    Err(TryRecvError::Closed) => {
                        self.done.set(true);
                        break 'inner;
                    }
                }
            }

            let mut cursor = self.cursor.get();
            let mut last_seq = self.last_seq.get().unwrap();
            {
                let mut map = self.received_data.borrow_mut();

                while let Some(packet) = map.get(&last_seq) {
                    let packet_data = packet.get_data();
                    let data = &packet_data[cursor..];

                    let written = user_data.write(data).unwrap();

                    if written != data.len() {
                        self.cursor.set(cursor + written);
                        self.last_seq.set(Some(last_seq));
                        return Ok(user_data.position() as usize);
                    }

                    map.retain(|k, _| k.cmp_greater(last_seq));

                    cursor = 0;
                    last_seq += 1;
                }
            }

            self.cursor.set(0);
            self.last_seq.set(Some(last_seq));

            if user_data.position() > 0 {
                return Ok(user_data.position() as usize);
            }

            if self.done.get() {
                return Err(std::io::ErrorKind::ConnectionAborted.into());
            }

            if let Ok(data) = self.receive.recv().await {
                current.replace(Ok(data));
            }
        }
    }

    pub async fn write(&self, data: &[u8]) {
        let data = Vec::from_slice(data).into_boxed_slice();
        self.writer_user_command
            .send(UtpEvent::UserWrite { data })
            .await
            .unwrap();
    }

    pub async fn wait_for_termination(&self) {
        if let Ok(ReceivedData::Done) = self.receive.recv().await {
            println!("Done received");
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::{Packet, ReceivedData, UtpStream};
    use async_channel::bounded;
    use shared_arena::Arena;

    #[tokio::test]
    async fn utp_read() {
        let arena = Arena::new();

        let (sender, recv) = bounded(10);
        let (writer, _) = bounded(10);
        let stream = UtpStream::new(recv, writer);

        sender
            .try_send(ReceivedData::FirstSequence { seq: 1.into() })
            .unwrap();

        let mut packet = arena.alloc(Packet::new(&[1, 2, 3]));
        packet.set_seq_number(1.into());
        sender.try_send(ReceivedData::Data { packet }).unwrap();

        let mut packet = arena.alloc(Packet::new(&[7, 8, 9]));
        packet.set_seq_number(3.into());
        sender.try_send(ReceivedData::Data { packet }).unwrap();

        let mut packet = arena.alloc(Packet::new(&[4, 5, 6]));
        packet.set_seq_number(2.into());
        sender.try_send(ReceivedData::Data { packet }).unwrap();

        let mut buffer = [0; 64];
        stream.read(&mut buffer).await;

        assert_eq!(&buffer[..9], &[1, 2, 3, 4, 5, 6, 7, 8, 9]);

        sender.try_send(ReceivedData::Done).unwrap();
        assert!(stream.read(&mut buffer).await.is_err());
    }

    #[tokio::test]
    async fn utp_read_partial() {
        let arena = Arena::new();

        let (sender, recv) = bounded(10);
        let (writer, _) = bounded(10);
        let stream = UtpStream::new(recv, writer);

        sender
            .try_send(ReceivedData::FirstSequence { seq: 1.into() })
            .unwrap();

        let mut packet = arena.alloc(Packet::new(&[1, 2, 3, 4, 5, 6, 7, 8, 9]));
        packet.set_seq_number(1.into());
        sender.try_send(ReceivedData::Data { packet }).unwrap();

        let mut packet = arena.alloc(Packet::new(&[13, 14, 15]));
        packet.set_seq_number(3.into());
        sender.try_send(ReceivedData::Data { packet }).unwrap();

        let mut packet = arena.alloc(Packet::new(&[10, 11, 12]));
        packet.set_seq_number(2.into());
        sender.try_send(ReceivedData::Data { packet }).unwrap();

        let mut buffer = [0; 4];

        stream.read(&mut buffer).await;
        assert_eq!(&buffer[..4], &[1, 2, 3, 4]);

        stream.read(&mut buffer).await;
        assert_eq!(&buffer[..4], &[5, 6, 7, 8]);

        stream.read(&mut buffer).await;
        assert_eq!(&buffer[..4], &[9, 10, 11, 12]);

        stream.read(&mut buffer).await;
        assert_eq!(&buffer[..3], &[13, 14, 15]);

        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(50));
            let mut packet = arena.alloc(Packet::new(&[100, 101, 102]));
            packet.set_seq_number(4.into());
            sender.try_send(ReceivedData::Data { packet }).unwrap();

            tokio::time::sleep(Duration::from_millis(50));
        });

        stream.read(&mut buffer).await;
        assert_eq!(&buffer[..3], &[100, 101, 102]);

        let res = stream.read(&mut buffer).await;
        assert!(res.is_err());
    }

    #[tokio::test]
    async fn utp_read_close_on_await() {
        let (sender, recv) = bounded(10);
        let (writer, _) = bounded(10);
        let stream = UtpStream::new(recv, writer);

        sender
            .try_send(ReceivedData::FirstSequence { seq: 1.into() })
            .unwrap();

        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(50));
            drop(sender);
        });

        let mut buffer = [0; 4];
        let res = stream.read(&mut buffer).await;
        assert!(res.is_err());
    }

    // #[tokio::test]
    // #[should_panic(expected = "send first sequence")]
    // async fn utp_read_no_init() {
    //     let arena = Arena::new();

    //     let (sender, recv) = bounded(10);
    //     let (writer, _) = bounded(10);
    //     let stream = UtpStream::new(recv, writer);

    //     let mut packet = arena.alloc(Packet::new(&[1,2,3]));
    //     packet.seq_number = 1.into();
    //     sender.try_send(ReceivedData::Data { packet }).unwrap();

    //     let mut buffer: Vec<u8> = vec![64; 0];
    //     stream.read(buffer.as_mut_slice()).await.unwrap();
    // }
}
