use std::{
    collections::VecDeque,
    convert::TryInto,
    os::unix::io::{AsRawFd, RawFd},
    ptr::NonNull,
};

use kv_log_macro::error;

use crate::utils::{Map, NoHash};

use super::ring::{Completed, IoUring, Operation};

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

pub struct FilesUring<T> {
    io_uring: IoUring,
    ring_id: u32,
    pending: Map<RingId, Pending<T>>,
    completed: VecDeque<Completed>,
    /// +1 when an entry is added to the sq (submitted or not)
    /// -1 when an entry is read from the cq
    in_flight: u32,
    need_submit: bool,
    registered_files: Vec<RawFd>,
    /// map fd to index in registered_files
    registered_files_map: Map<RawFd, usize>,
    should_register_files: bool,
}

unsafe impl<T> Send for FilesUring<T> {}

#[derive(Copy, Clone)]
pub(super) enum OpKind {
    Write,
    Read,
}

struct Pending<T> {
    op: PendingOperation,
    user_data: Option<T>,
}

enum PendingOperation {
    ReadWrite {
        fd: FileDescriptor,
        data: NonNull<u8>,
        data_length: usize,
        /// number of bytes read/written
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

impl<T> FilesUring<T> {
    pub fn new(ring_size: u32) -> std::io::Result<FilesUring<T>> {
        let iou = IoUring::new(ring_size)?;

        let mut should_register_files = true;

        // TODO:
        // Make this configurable and respect RLIMIT_NOFILE
        let registered_files = vec![-1; 512];
        if iou.register_files(&registered_files).is_err() {
            should_register_files = false;
        }

        Ok(Self {
            io_uring: iou,
            ring_id: 10,
            pending: Map::with_capacity_and_hasher(16, NoHash::default()),
            completed: VecDeque::with_capacity(64),
            in_flight: 0,
            need_submit: false,
            registered_files,
            registered_files_map: Map::default(),
            should_register_files,
        })
    }

    pub fn unregister_files<'a, F: 'a, I>(&mut self, files: I)
    where
        F: AsRawFd,
        I: Iterator<Item = &'a F>,
    {
        let mut min = self.registered_files.len();
        let mut max = 0;

        assert!(min > max);

        for f in files {
            let fd = f.as_raw_fd();
            if let Some(index) = self.registered_files_map.remove(&fd) {
                self.registered_files[index] = -1;
                min = index.min(min);
                max = index.max(max);
            }
        }

        if min <= max {
            let slice = &self.registered_files[min..max + 1];
            let offset = min;
            self.io_uring.register_file_update(slice, offset).unwrap();
        }
    }

    fn ensure_can_push(&mut self) {
        let cq_entries = self.io_uring.cq_entries;
        let sq_entries = self.io_uring.sq_entries;

        if self.in_flight == cq_entries {
            // We made the maximum number of request for the completion
            // queue. We can't push more request, we need to wait for
            // at least one to complete before being able to push.
            let completed = self.io_uring.wait_pop().unwrap();
            self.in_flight = self.in_flight.checked_sub(1).unwrap();
            self.completed.push_back(completed);
        }

        let sq_pending = self.io_uring.sq_pending();

        if sq_pending == sq_entries {
            // The submission queue is full, need to submit
            self.submit();
        }
    }

    pub fn in_flight(&self) -> u32 {
        let in_completed: u32 = self.completed.len().try_into().unwrap();
        self.in_flight + in_completed
    }

    pub fn submit(&mut self) {
        if self.need_submit {
            let nfree_cq = self.io_uring.cq_entries - self.in_flight;
            // Do not submit more than what is available in cq
            if nfree_cq > 0 {
                self.io_uring.submit(nfree_cq).unwrap();
                self.need_submit = self.io_uring.sq_pending() != 0;
            }
        }
    }

    fn find_or_register_file(&mut self, fd: RawFd) -> FileDescriptor {
        if !self.should_register_files {
            return FileDescriptor::Fd(fd);
        }

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

    fn write<F>(&mut self, fd: &F, offset: usize, data: &[u8])
    where
        F: AsRawFd,
    {
        unsafe { self.write_with_data(fd, offset, data, None) }
    }

    /// # Safety
    /// The buffer must live until the request is completed
    pub unsafe fn write_with_data<F, D>(&mut self, fd: &F, offset: usize, data: &[u8], user_data: D)
    where
        F: AsRawFd,
        D: Into<Option<T>>,
    {
        let fd = self.find_or_register_file(fd.as_raw_fd());
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
        self.pending.insert(
            id,
            Pending {
                user_data: user_data.into(),
                op: PendingOperation::ReadWrite {
                    fd,
                    data: NonNull::new(data.as_ptr() as *mut _).unwrap(),
                    data_length: data.len(),
                    nbytes_processed: 0,
                    offset,
                    neof: 0,
                    kind: OpKind::Write,
                },
            },
        );
    }

    fn read<F>(&mut self, fd: &F, offset: usize, data: &mut [u8])
    where
        F: AsRawFd,
    {
        unsafe { self.read_with_data(fd, offset, data, None) }
    }

    /// # Safety
    /// The buffer must live until the request is completed
    pub unsafe fn read_with_data<F, D>(
        &mut self,
        fd: &F,
        offset: usize,
        data: &mut [u8],
        user_data: D,
    ) where
        F: AsRawFd,
        D: Into<Option<T>>,
    {
        let fd = self.find_or_register_file(fd.as_raw_fd());
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
        self.pending.insert(
            id,
            Pending {
                user_data: user_data.into(),
                op: PendingOperation::ReadWrite {
                    fd,
                    data: NonNull::new(data.as_ptr() as *mut _).unwrap(),
                    data_length: data.len(),
                    nbytes_processed: 0,
                    offset,
                    neof: 0,
                    kind: OpKind::Read,
                },
            },
        );
    }

    pub fn get_completed(&mut self) -> Option<(Option<T>, std::io::Result<()>)> {
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

    pub fn wait_completed(&mut self) -> Option<(Option<T>, std::io::Result<()>)> {
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

    pub fn block(&mut self) {
        if !self.completed.is_empty() {
            return;
        }

        if self.in_flight == 0 {
            return;
        }

        let completed = self.io_uring.wait_pop().unwrap();
        self.in_flight = self.in_flight.checked_sub(1).unwrap();
        self.completed.push_back(completed);
    }

    fn process_completed(
        &mut self,
        completed: Completed,
    ) -> Option<(Option<T>, std::io::Result<()>)> {
        // println!("Completed {:?}", completed);

        let written = completed.result;
        let id: RingId = completed.into();

        if written < 0 {
            // Error

            let Pending { user_data, .. } = self.pending.remove(&id).unwrap();
            error!("Operation failed error_code={:?}", written);
            return Some((user_data, Err(std::io::ErrorKind::Other.into())));
        }

        let Pending { op: pending, .. } = self.pending.get_mut(&id).unwrap();

        match pending {
            PendingOperation::Register => {
                let Pending { user_data, .. } = self.pending.remove(&id).unwrap();

                Some((user_data, Ok(())))
            }
            PendingOperation::ReadWrite {
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

                    let Pending { user_data, .. } = self.pending.remove(&id).unwrap();
                    Some((user_data, Ok(())))
                } else {
                    // Partial read/write, make another request
                    // See https://github.com/axboe/liburing/issues/78
                    let nbytes_processed = *nbytes_processed;
                    let data_length = *data_length;

                    assert!(nbytes_processed < data_length);

                    if written == 0 {
                        // Try twice
                        // https://github.com/facebook/rocksdb/pull/6441#issuecomment-589843435
                        if *neof == 1 {
                            // EOF reached

                            let Pending { user_data, .. } = self.pending.remove(&id).unwrap();
                            return Some((user_data, Ok(())));
                        }
                        *neof += 1;
                    }

                    // Safety:
                    // The new slice doesn't go past the end of the original slice
                    let data = unsafe {
                        std::slice::from_raw_parts_mut(
                            data.as_ptr().add(nbytes_processed),
                            data_length - nbytes_processed,
                        )
                    };
                    let fd = *fd;
                    let offset = *offset + nbytes_processed;
                    let kind = *kind;

                    self.ensure_can_push();
                    self.in_flight += 1;
                    self.need_submit = true;

                    // Read/Write the remaining data
                    self.io_uring
                        .push_entry(Operation::from((kind, id, fd, offset, data)))
                        .unwrap();

                    self.submit();

                    None
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::FilesUring;

    #[test]
    #[cfg_attr(miri, ignore)] // Miri doesn't support io_uring
    fn simple_file() {
        crate::logger::start();

        let fd = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .read(true)
            .open("coucou.txt")
            .unwrap();

        let mut file = match FilesUring::<()>::new(4) {
            Ok(file) => file,
            Err(_) => return,
        };

        let mut vec = Vec::with_capacity(0x4000 * 25);
        for n in 0..vec.capacity() {
            vec.push((n & 0xFF) as u8);
        }

        file.write(&fd, 0, vec.as_slice());
        assert_eq!(file.in_flight(), 1);

        let res = file.wait_completed();
        assert!(res.unwrap().1.is_ok());
        let res = file.wait_completed();
        assert!(res.is_none());

        assert_eq!(file.in_flight(), 0);

        file.unregister_files(std::iter::once(&fd));
    }

    #[test]
    #[cfg_attr(miri, ignore)] // Miri doesn't support io_uring
    fn multiple_write() {
        crate::logger::start();

        let fd = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .read(true)
            .truncate(true)
            .open("coucou2.txt")
            .unwrap();

        let mut file = match FilesUring::<()>::new(4) {
            Ok(file) => file,
            Err(_) => return,
        };

        const SIZE: usize = 0x4000 * 250;

        let mut data = Vec::with_capacity(SIZE);
        for _ in 0..data.capacity() {
            data.push(fastrand::u8(..));
        }

        for (index, byte) in data.chunks(2).enumerate() {
            file.write(&fd, index * 2, byte);
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
            file.read(&fd, index * 2, chunk);
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
        assert_eq!(read, data);
    }
}
