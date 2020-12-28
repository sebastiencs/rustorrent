use std::{
    collections::{HashMap, VecDeque},
    convert::TryInto,
    io::ErrorKind::UnexpectedEof,
    ptr::NonNull,
};

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

#[derive(Debug, Copy, Clone)]
pub enum FileDescriptor {
    Fd(i32),
    RegisteredIndex(i32),
}

struct File {
    io_uring: IoUring,
    ring_id: u32,
    pending_write: Map<RingId, Pending>,
    completed: VecDeque<Completed>,
    in_flight: usize,
    need_submit: bool,
    registered_files: Vec<i32>,
    /// map fd to index in registered_files
    registered_files_map: Map<i32, usize>,
}

#[derive(Copy, Clone)]
enum OpKind {
    Write,
    Read,
}

enum Pending {
    ReadWrite {
        fd: FileDescriptor,
        data: NonNull<u8>,
        data_length: usize,
        // number of bytes read/written
        nbytes_processed: usize,
        offset: usize,
        /// Number of EOF (res == 0) received
        /// Consider EOF reached after 2
        /// https://github.com/facebook/rocksdb/pull/6441#issuecomment-589843435
        neof: u8,
        kind: OpKind,
    },
    Register,
}

impl File {
    pub fn new() -> std::io::Result<File> {
        let iou = IoUring::new(4)?;

        // TODO:
        // Make this configurable and respect RLIMIT_NOFILE
        let registered_files = vec![-1; 512];
        iou.register_files(&registered_files).unwrap();

        Ok(Self {
            io_uring: iou,
            ring_id: 10,
            pending_write: HashMap::default(),
            completed: VecDeque::with_capacity(64),
            in_flight: 0,
            need_submit: false,
            registered_files,
            registered_files_map: Map::default(),
        })
    }

    fn ensure_can_push(&mut self) {
        let sq_available = self.io_uring.sq_available();
        let cq_entries = self.io_uring.cq_entries as usize;

        if sq_available == 0 {
            // The submission queue is full, need to submit
            self.submit();
        }

        // let sq_available = sq.available();

        if sq_available == 0 || self.in_flight == cq_entries {
            // println!("LAAA {:?} {:?} {:?}", sq_available, self.in_flight, cq_entries);
            // We made the maximum number of request for the completion
            // queue. We can't push more request, we need to wait for
            // at least one to complete before being able to push.
            let completed = self.io_uring.wait_pop().unwrap();
            self.in_flight = self.in_flight.checked_sub(1).unwrap();
            self.completed.push_back(completed);
        }
    }

    pub fn in_flight(&self) -> usize {
        self.in_flight
    }

    pub fn submit(&mut self) {
        if self.need_submit {
            self.io_uring.submit().unwrap();
            self.need_submit = false;
        }
    }

    fn find_or_register_file(&mut self, fd: i32) -> FileDescriptor {
        if let Some(index) = self.registered_files_map.get(&fd) {
            return FileDescriptor::RegisteredIndex((*index).try_into().unwrap());
        }

        let new_index = match self.registered_files.iter().position(|file| *file == -1) {
            Some(index) => index,
            None => return FileDescriptor::Fd(fd),
        };

        self.registered_files[new_index] = fd;
        self.registered_files_map.insert(fd, new_index);

        let slice = &self.registered_files[new_index..new_index + 1];
        let offset = new_index;
        self.io_uring.register_file_update(slice, offset).unwrap();

        FileDescriptor::RegisteredIndex(new_index.try_into().unwrap())
    }

    pub fn write(&mut self, fd: i32, offset: usize, data: &[u8]) {
        let fd = self.find_or_register_file(fd);
        let id = self.ring_id.into();

        self.ensure_can_push();

        self.io_uring
            .push_entry(Operation::Write {
                id,
                fd,
                offset,
                data,
            })
            .unwrap();

        self.need_submit = true;
        self.ring_id = self.ring_id.wrapping_add(1);
        self.in_flight += 1;
        self.pending_write.insert(
            id,
            Pending::ReadWrite {
                fd,
                data: NonNull::new(data.as_ptr() as *mut _).unwrap(),
                data_length: data.len(),
                nbytes_processed: 0,
                offset,
                neof: 0,
                kind: OpKind::Write,
            },
        );
    }

    pub fn read(&mut self, fd: i32, offset: usize, data: &mut [u8]) {
        let fd = self.find_or_register_file(fd);
        let id = self.ring_id.into();

        self.ensure_can_push();

        self.io_uring
            .push_entry(Operation::Read {
                id,
                fd,
                offset,
                data,
            })
            .unwrap();

        self.need_submit = true;
        self.ring_id = self.ring_id.wrapping_add(1);
        self.in_flight += 1;
        self.pending_write.insert(
            id,
            Pending::ReadWrite {
                fd,
                data: NonNull::new(data.as_ptr() as *mut _).unwrap(),
                data_length: data.len(),
                nbytes_processed: 0,
                offset,
                neof: 0,
                kind: OpKind::Read,
            },
        );
    }

    pub fn get_completed(&mut self) -> Option<(RingId, std::io::Result<()>)> {
        loop {
            let completed = self.completed.pop_front().or_else(|| {
                self.submit();
                let completed = self.io_uring.pop();
                if completed.is_some() {
                    self.in_flight = self.in_flight.checked_sub(1).unwrap();
                }
                completed
            })?;

            if let Some(res) = self.process_completed(completed) {
                return Some(res);
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
                    let completed = self.io_uring.wait_pop().ok();
                    if completed.is_some() {
                        self.in_flight = self.in_flight.checked_sub(1).unwrap();
                    }
                    completed
                } else {
                    None
                }
            })?;

            if let Some(res) = self.process_completed(completed) {
                return Some(res);
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

        match pending {
            Pending::Register => Some((id, Ok(()))),
            Pending::ReadWrite {
                fd,
                data,
                data_length,
                nbytes_processed,
                offset,
                neof,
                kind,
            } => {
                *nbytes_processed += written as usize;

                if nbytes_processed == data_length {
                    // All data written

                    self.pending_write.remove(&id).unwrap();
                    Some((id, Ok(())))
                } else {
                    // Partial read/write, make another request
                    // See https://github.com/axboe/liburing/issues/78

                    if written == 0 {
                        // Try twice
                        // https://github.com/facebook/rocksdb/pull/6441#issuecomment-589843435
                        if *neof == 1 {
                            // EOF reached

                            self.pending_write.remove(&id).unwrap();
                            return Some((id, Err(UnexpectedEof.into())));
                        }
                        *neof += 1;
                    }
                    let data = unsafe {
                        std::slice::from_raw_parts_mut(
                            data.as_ptr().add(*nbytes_processed as usize),
                            *data_length - *nbytes_processed,
                        )
                    };
                    let fd = *fd;
                    let offset = *offset + *nbytes_processed;
                    let kind = *kind;

                    // For &mut self aliasing
                    drop(pending);

                    self.ensure_can_push();
                    self.in_flight += 1;

                    // Read/Write the remaining data
                    self.io_uring
                        .push_entry(match kind {
                            OpKind::Write => Operation::Write {
                                id,
                                fd,
                                offset,
                                data,
                            },
                            OpKind::Read => Operation::Read {
                                id,
                                fd,
                                offset,
                                data,
                            },
                        })
                        .unwrap();
                    self.io_uring.submit().unwrap();
                    self.need_submit = false;

                    None
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{File, IoUring};
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

        let res = file.wait_completed();
        println!("RES={:?}", res);
        assert!(res.unwrap().1.is_ok());
        let res = file.wait_completed();
        println!("RES={:?}", res);
        assert!(res.is_none());

        assert_eq!(file.in_flight(), 0);
    }

    #[test]
    #[cfg_attr(miri, ignore)] // Miri doesn't support io_uring
    fn multiple_write() {
        let fd = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .read(true)
            .truncate(true)
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
            assert!(res.1.is_ok());
            results.push(res);
        }
        while let Some(res) = file.wait_completed() {
            assert!(res.1.is_ok());
            results.push(res);
        }

        assert_eq!(results.len(), SIZE / 2);

        let result_file = std::fs::read("coucou2.txt").unwrap();
        assert_eq!(result_file, data);

        let mut read = vec![0; SIZE];

        for (index, chunk) in read.chunks_mut(2).enumerate() {
            file.read(fd.as_raw_fd(), index * 2, chunk);
        }

        let mut results = Vec::new();
        while let Some(res) = file.get_completed() {
            assert!(res.1.is_ok());
            results.push(res);
        }
        while let Some(res) = file.wait_completed() {
            assert!(res.1.is_ok());
            results.push(res);
        }

        assert_eq!(results.len(), SIZE / 2);

        // println!("RES={:?}", res);

        assert_eq!(read, data);
    }
}
