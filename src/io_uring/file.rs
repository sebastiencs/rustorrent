use std::{collections::{HashMap, VecDeque}, convert::TryInto, io::ErrorKind::UnexpectedEof, ptr::NonNull};

use crate::utils::Map;

use super::{Completed, IoUring, Operation};

#[derive(Debug, Copy, Clone, Hash, Eq, PartialEq)]
pub struct RingId(u32);

impl std::fmt::Display for RingId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "RingId {}", self.0)
    }
}

impl From<Completed> for RingId {
    fn from(c: Completed) -> Self {
        let id = c.user_data & 0xFFFFFFFF;
        Self(id.try_into().unwrap())
    }
}

impl From<u32> for RingId {
    fn from(n: u32) -> Self {
        Self(n)
    }
}

impl From<RingId> for u64 {
    fn from(id: RingId) -> Self {
        id.0 as u64
    }
}

struct File {
    io_uring: IoUring,
    ring_id: u32,
    pending_write: Map<RingId, PendingWrite>,
    completed: VecDeque<Completed>,
    in_flight: usize,
    need_submit: bool,
}

struct PendingWrite {
    fd: i32,
    data: NonNull<u8>,
    data_length: usize,
    written: usize,
    offset: usize,
    /// Number of EOF (res == 0) received
    /// Consider EOF reached after 2
    /// https://github.com/facebook/rocksdb/pull/6441#issuecomment-589843435
    neof: u8,
}

impl File {
    pub fn new() -> std::io::Result<File> {
        Ok(Self {
            io_uring: IoUring::new(4)?,
            ring_id: 10,
            pending_write: HashMap::default(),
            completed: VecDeque::with_capacity(64),
            in_flight: 0,
            need_submit: false,
        })
    }

    fn ensure_can_push(&mut self) {
        let sq = self.io_uring.sq();
        let cq = self.io_uring.cq();

        let sq_available = sq.available();
        let cq_entries = *cq.entries as usize;

        if sq_available == 0 {
            // The submission queue is full, need to submit
            self.submit();
            // sq.submit().ok();
            // self.need_submit = false;
            // TODO: Should we wait for a completion here ?
        }

        // let sq_available = sq.available();

        if sq_available == 0 || self.in_flight == cq_entries {
            // println!("LAAA {:?} {:?} {:?}", sq_available, self.in_flight, cq_entries);
            // We made the maximum number of request for the completion
            // queue. We can't push more request, we need to wait for
            // at least one to complete before being able to push.
            let completed = self.io_uring.cq().wait_pop().unwrap();
            self.in_flight = self.in_flight.checked_sub(1).unwrap();
            self.completed.push_back(completed);
        }
    }

    pub fn in_flight(&self) -> usize {
        self.in_flight
    }

    pub fn submit(&mut self) {
        if self.need_submit {
            self.io_uring.sq().submit().unwrap();
            self.need_submit = false;
        }
    }

    pub fn write(&mut self, fd: i32, offset: usize, data: &[u8]) {

        self.ensure_can_push();

        let sq = self.io_uring.sq();
        let id = self.ring_id.into();

        sq.push_entry(Operation::Write {
            id,
            fd,
            offset,
            data,
        }).unwrap();

        self.need_submit = true;
        self.ring_id = self.ring_id.wrapping_add(1);
        self.in_flight += 1;
        self.pending_write.insert(id, PendingWrite {
            fd,
            data: NonNull::new(data.as_ptr() as *mut _).unwrap(),
            data_length: data.len(),
            written: 0,
            offset,
            neof: 0,
        });
    }

    pub fn get_completed(&mut self) -> Option<(RingId, std::io::Result<()>)> {
        loop {
            let completed = self.completed.pop_front().or_else(|| {
                self.submit();
                let completed = self.io_uring.cq().pop();
                if completed.is_some() {
                    self.in_flight = self.in_flight.checked_sub(1).unwrap();
                }
                completed
            })?;

            if let Some(res) = self.process_completed(completed) {
                return Some(res)
            }
        }
    }

    pub fn wait_completed(&mut self) -> Option<(RingId, std::io::Result<()>)> {
        loop {
            let completed = self.completed.pop_front().or_else(|| {
                self.submit();
                // Make sure to not block forever, wait only when there are
                // in flight requests
                if self.in_flight > 0 {
                    let completed = self.io_uring.cq().wait_pop().ok();
                    if completed.is_some() {
                        self.in_flight = self.in_flight.checked_sub(1).unwrap();
                    }
                    completed
                } else {
                    None
                }
            })?;

            if let Some(res) = self.process_completed(completed) {
                return Some(res)
            }
        }
    }

    fn process_completed(&mut self, completed: Completed) -> Option<(RingId, std::io::Result<()>)> {
        // println!("Completed {:?}", completed);

        let written = completed.result;
        let id: RingId = completed.into();

        if written < 0 {
            // Error

            self.pending_write.remove(&id).unwrap();
            return Some((id, Err(std::io::ErrorKind::Other.into())));
        }

        let mut pending = self.pending_write.get_mut(&id).unwrap();
        pending.written += written as usize;

        if pending.written == pending.data_length {
            // All data written

            self.pending_write.remove(&id).unwrap();
            Some((id, Ok(())))
        } else {
            // Partial read/write, make another request
            // See https://github.com/axboe/liburing/issues/78

            if written == 0 {
                // https://github.com/facebook/rocksdb/pull/6441#issuecomment-589843435
                if pending.neof == 1 {
                    // EOF reached

                    self.pending_write.remove(&id).unwrap();
                    return Some((id, Err(UnexpectedEof.into())));
                }
                pending.neof += 1;
            }
            let data = unsafe {
                std::slice::from_raw_parts(
                    pending.data.as_ptr().add(pending.written as usize),
                    pending.data_length - pending.written
                )
            };
            let fd = pending.fd;
            let offset = pending.offset + pending.written;
            drop(pending); // For &mut self aliasing

            self.ensure_can_push();
            self.in_flight += 1;

            // Write the remaining data
            let sq = self.io_uring.sq();
            sq.push_entry(Operation::Write {
                id,
                fd,
                offset,
                data,
            }).unwrap();
            sq.submit().unwrap();
            self.need_submit = false;

            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{IoUring, File};
    use std::os::unix::io::AsRawFd;

    #[test]
    #[cfg_attr(miri, ignore)] // Miri doesn't support io_uring
    fn simple_file() {

        let fd = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .read(true)
            .open("coucou.txt")
            .unwrap();

        let mut file = File::new().unwrap();

        let mut vec = Vec::with_capacity(0x4000 * 25);
        for n in 0..vec.capacity() {
            vec.push((n & 0xFF) as u8);
        }

        file.write(fd.as_raw_fd(), 0, vec.as_slice());
        assert_eq!(file.in_flight(), 1);

        file.wait_completed();
        assert_eq!(file.in_flight(), 0);
    }

    #[test]
    #[cfg_attr(miri, ignore)] // Miri doesn't support io_uring
    fn multiple_write() {

        let fd = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .read(true)
            .open("coucou2.txt")
            .unwrap();

        let mut file = File::new().unwrap();

        const SIZE: usize = 0x4000 * 25;

        let mut data = Vec::with_capacity(SIZE);
        for n in 0..data.capacity() {
            data.push(fastrand::u8(..));
        }

        for (index, byte) in data.chunks(2).enumerate() {
            file.write(fd.as_raw_fd(), index * 2, byte);
        }

        let mut results = Vec::new();
        while let Some(res) = file.get_completed() {
            results.push(res);
        }
        while let Some(res) = file.wait_completed() {
            results.push(res);
        }

        assert_eq!(results.len(), SIZE / 2);

        let result_file = std::fs::read("coucou2.txt").unwrap();
        assert_eq!(result_file, data);
    }
}
