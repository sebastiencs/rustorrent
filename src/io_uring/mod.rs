use std::{
    cell::UnsafeCell,
    fmt::Debug,
    io::IoSlice,
    os::unix::io::RawFd,
    path::Path,
    ptr::NonNull,
    sync::atomic::{
        AtomicU32,
        Ordering::{Acquire, Relaxed, Release},
    },
};

use bitflags::bitflags;
use libc::{c_int, c_long, c_uint, c_void, off_t, syscall};
use static_assertions::{assert_eq_size, assert_type_eq_all};

use self::file::RingId;

pub mod file;

const SYS_IO_URING_SETUP: c_long = 425;
const SYS_IO_URING_ENTER: c_long = 426;
const SYS_IO_URING_REGISTER: c_long = 427;

// Magic offsets for the application to mmap the data it needs
const IORING_OFF_SQ_RING: off_t = 0;
const IORING_OFF_CQ_RING: off_t = 0x8000000;
const IORING_OFF_SQES: off_t = 0x10000000;

const IORING_OP_NOP: u8 = 0;
const IORING_OP_READV: u8 = 1;
const IORING_OP_WRITEV: u8 = 2;
const IORING_OP_FSYNC: u8 = 3;
const IORING_OP_READ_FIXED: u8 = 4;
const IORING_OP_WRITE_FIXED: u8 = 5;
const IORING_OP_POLL_ADD: u8 = 6;
const IORING_OP_POLL_REMOVE: u8 = 7;
const IORING_OP_SYNC_FILE_RANGE: u8 = 8;
const IORING_OP_SENDMSG: u8 = 9;
const IORING_OP_RECVMSG: u8 = 10;
const IORING_OP_TIMEOUT: u8 = 11;
const IORING_OP_TIMEOUT_REMOVE: u8 = 12;
const IORING_OP_ACCEPT: u8 = 13;
const IORING_OP_ASYNC_CANCEL: u8 = 14;
const IORING_OP_LINK_TIMEOUT: u8 = 15;
const IORING_OP_CONNECT: u8 = 16;
const IORING_OP_FALLOCATE: u8 = 17;
const IORING_OP_OPENAT: u8 = 18;
const IORING_OP_CLOSE: u8 = 19;
const IORING_OP_FILES_UPDATE: u8 = 20;
const IORING_OP_STATX: u8 = 21;
const IORING_OP_READ: u8 = 22;
const IORING_OP_WRITE: u8 = 23;
const IORING_OP_FADVISE: u8 = 24;
const IORING_OP_MADVISE: u8 = 25;
const IORING_OP_SEND: u8 = 26;
const IORING_OP_RECV: u8 = 27;
const IORING_OP_OPENAT2: u8 = 28;
const IORING_OP_EPOLL_CTL: u8 = 29;
const IORING_OP_SPLICE: u8 = 30;
const IORING_OP_PROVIDE_BUFFERS: u8 = 31;
const IORING_OP_REMOVE_BUFFERS: u8 = 32;
const IORING_OP_TEE: u8 = 33;
const IORING_OP_SHUTDOWN: u8 = 34;
const IORING_OP_RENAMEAT: u8 = 35;
const IORING_OP_UNLINKAT: u8 = 36;

const IORING_ENTER_GETEVENTS: u8 = 1 << 0;
const IORING_ENTER_SQ_WAKEUP: u8 = 1 << 1;
const IORING_ENTER_SQ_WAIT: u8 = 1 << 2;
const IORING_ENTER_EXT_ARG: u8 = 1 << 3;

bitflags! {
    struct FeaturesFlags: u32 {
        const IORING_FEAT_SINGLE_MMAP = 1 << 0;
        const IORING_FEAT_NODROP = 1 << 1;
        const IORING_FEAT_SUBMIT_STABLE = 1 << 2;
        const IORING_FEAT_RW_CUR_POS = 1 << 3;
        const IORING_FEAT_CUR_PERSONALITY = 1 << 4;
        const IORING_FEAT_FAST_POLL = 1 << 5;
        const IORING_FEAT_POLL_32BITS = 1 << 6;
        const IORING_FEAT_SQPOLL_NONFIXED = 1 << 7;
        const IORING_FEAT_EXT_ARG = 1 << 8;
    }
}

#[derive(Debug, Clone)]
#[repr(C)]
struct CompletionQueueEntry {
    user_data: u64,
    res: i32,
    flags: u32,
}

assert_eq_size!(CompletionQueueEntry, [u8; 16]);

#[allow(non_camel_case_types)]
#[derive(Debug, Clone)]
pub(super) struct io_sqring_offsets {
    head: u32,
    tail: u32,
    ring_mask: u32,
    ring_entries: u32,
    flags: u32,
    dropped: u32,
    array: u32,
    resv: [u32; 3],
}

#[allow(non_camel_case_types)]
#[derive(Debug, Clone)]
pub(super) struct io_cqring_offsets {
    head: u32,
    tail: u32,
    ring_mask: u32,
    ring_entries: u32,
    overflow: u32,
    cqes: u32,
    flags: u32,
    resv: [u32; 3],
}

#[allow(non_camel_case_types)]
#[derive(Debug, Clone)]
pub(super) struct io_uring_params {
    sq_entries: u32,
    cq_entries: u32,
    flags: u32,
    sq_thread_cpu: u32,
    sq_thread_idle: u32,
    features: u32,
    resv: [u32; 4],
    sq_off: io_sqring_offsets,
    cq_off: io_cqring_offsets,
}

assert_eq_size!(io_uring_params, [u8; 120]);
assert_eq_size!(io_cqring_offsets, [u8; 40]);
assert_eq_size!(io_sqring_offsets, [u8; 40]);
assert_type_eq_all!(c_uint, u32);
assert_type_eq_all!(c_int, i32);

#[derive(Debug, Copy, Clone)]
pub(super) struct IoUringFd(RawFd);

pub(super) fn io_uring_setup(
    entries: u32,
    params: &mut io_uring_params,
) -> std::io::Result<IoUringFd> {
    let fd = unsafe {
        // int io_uring_setup(u32 entries, struct io_uring_params *p);
        syscall(SYS_IO_URING_SETUP, entries, params as *mut io_uring_params)
    };

    if fd < 0 {
        return Err(std::io::Error::last_os_error());
    }

    Ok(IoUringFd(fd as RawFd))
}

pub(super) fn io_uring_enter(
    fd: IoUringFd,
    to_submit: u32,
    min_complete: u32,
    flags: u32,
) -> std::io::Result<i32> {
    loop {
        let res = unsafe {
            // int io_uring_enter(unsigned int fd, unsigned int to_submit,
            //                    unsigned int min_complete, unsigned int flags,
            //                    sigset_t *sig);
            syscall(
                SYS_IO_URING_ENTER,
                fd.0,
                to_submit,
                min_complete,
                flags,
                std::ptr::null_mut::<()>(),
            ) as i32
        };

        if res < 0 {
            if let libc::EAGAIN | libc::EINTR = res {
                continue;
            };
            return Err(std::io::Error::last_os_error());
        }

        return Ok(res);
    }
}

pub(super) fn io_uring_register(fd: i32, opcode: u32, arg: *const (), nr_args: u32) -> isize {
    unsafe {
        // int io_uring_register(unsigned int fd, unsigned int opcode,
        //                       void *arg, unsigned int nr_args);
        syscall(SYS_IO_URING_REGISTER, fd, opcode, arg, nr_args) as isize
    }
}

pub(super) fn mmap<T>(
    io_uring_fd: &IoUringFd,
    length: u32,
    offset: off_t,
) -> std::io::Result<NonNull<T>> {
    let ptr = unsafe {
        libc::mmap(
            std::ptr::null_mut(),
            length as usize,
            libc::PROT_READ | libc::PROT_WRITE,
            libc::MAP_SHARED | libc::MAP_POPULATE,
            io_uring_fd.0,
            offset,
        )
    };

    if ptr == libc::MAP_FAILED {
        return Err(std::io::Error::last_os_error());
    }

    Ok(NonNull::new(ptr as *mut _).unwrap())
}

pub(super) fn munmap<T>(ptr: NonNull<T>, size: u32) -> std::io::Result<()> {
    let res = unsafe { libc::munmap(ptr.as_ptr() as *mut c_void, size as usize) };

    if res == -1 {
        return Err(std::io::Error::last_os_error());
    }

    Ok(())
}

fn display_io_uring_features(features: u32) {
    let flags = FeaturesFlags::from_bits_truncate(features);

    println!("[io_uring] Detected features: {:?}", flags);
}

union SubmissionFlags {
    fsync_flags: u32,
    poll_events: u16,
    poll32_events: u32,
    sync_range_flags: u32,
    msg_flags: u32,
    timeout_flags: u32,
    accept_flags: u32,
    cancel_flags: u32,
    open_flags: u32,
    statx_flags: u32,
    fadvise_advice: u32,
    splice_flags: u32,
    rename_flags: u32,
    unlink_flags: u32,
}

impl Debug for SubmissionFlags {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        unsafe { write!(f, "SubmissionFlags {:032b}", self.fsync_flags) }
    }
}

#[derive(Debug)]
#[repr(C)]
struct SubmissionQueueEntry {
    /// type of operation for this sqe
    opcode: u8,
    /// IOSQE_ flags
    iosqe_flags: u8,
    /// ioprio for the request
    ioprio: u16,
    /// file descriptor to do IO on
    fd: i32,
    /// off: offset into file
    off_addr2: u64,
    /// pointer to buffer or iovecs
    addr_splice_off_in: u64,
    len: u32,
    flags: SubmissionFlags,
    user_data: u64,
    buf_index: u16,
    personality: u16,
    splice_fd_in: u32,
    _pad: [u64; 2],
}

assert_eq_size!(SubmissionQueueEntry, [u8; 64]);
assert_eq_size!(SubmissionQueueEntry, UnsafeCell<SubmissionQueueEntry>);

#[derive(Debug)]
pub enum Operation<'a> {
    ReadFixed {
        fd: RawFd,
        offset: usize,
        length: usize,
        buf_index: u16,
    },
    Read {
        fd: RawFd,
        offset: usize,
        length: usize,
        iovecs: &'a [IoSlice<'a>],
    },
    Write {
        id: RingId,
        fd: RawFd,
        offset: usize,
        data: &'a [u8],
        //iovecs: &'a [IoSlice<'a>],
    },
    OpenAt {
        path: &'a Path,
    },
    TimeOut {
        spec: *mut libc::timespec,
    },
    NoOp,
}

impl SubmissionQueueEntry {
    fn zeroed(&mut self) {
        *self = unsafe { std::mem::zeroed() }
    }

    fn apply_operation(&mut self, op: Operation) {
        self.zeroed();

        match op {
            Operation::ReadFixed {
                fd,
                offset,
                length,
                buf_index,
            } => {
                self.fd = fd;
                self.opcode = IORING_OP_READ_FIXED;
                self.off_addr2 = offset as u64;
                // self.addr_splice_off_in = iovecs.as_ptr() as u64;
                self.buf_index = buf_index;
                self.len = length as u32;
            }
            Operation::Read {
                fd,
                offset,
                length,
                iovecs,
            } => {
                self.fd = fd;
                self.opcode = IORING_OP_READ;
                self.off_addr2 = offset as u64;
                self.addr_splice_off_in = iovecs.as_ptr() as u64;
                self.len = length as u32;
            }
            Operation::Write {
                id,
                fd,
                offset,
                data,
                //iovecs: _,
            } => {
                self.opcode = IORING_OP_WRITE;
                self.fd = fd;
                self.addr_splice_off_in = data.as_ptr() as u64;
                self.off_addr2 = offset as u64;
                //self.flags.open_flags = flags as u32;
                self.len = data.len() as u32;
                self.user_data = id.into();
            }
            Operation::OpenAt { path } => {
                let path = path.to_str().unwrap().as_ptr();
                let flags = libc::O_CREAT | libc::O_RDWR;
                let mode = libc::S_IRUSR | libc::S_IWUSR | libc::S_IRGRP | libc::S_IROTH;

                self.opcode = IORING_OP_OPENAT;
                self.fd = libc::AT_FDCWD;
                self.addr_splice_off_in = path as u64;
                self.flags.open_flags = flags as u32;
                self.len = mode as u32;
            }
            Operation::TimeOut { spec } => {
                self.opcode = IORING_OP_TIMEOUT;
                self.addr_splice_off_in = spec as u64;
                self.len = 1;
            }
            Operation::NoOp => {
                self.opcode = IORING_OP_NOP;
            }
        }
    }
}

pub struct SubmissionQueue<'ring> {
    head: &'ring AtomicU32,
    tail: &'ring AtomicU32,
    mask: &'ring u32,
    entries: &'ring u32,
    // flags: &'ring AtomicU32,
    // dropped: &'ring u32,
    // array: &'ring [AtomicU32],
    sqes: &'ring [UnsafeCell<SubmissionQueueEntry>],
    // sqe_head: Cell<u32>,
    // sqe_tail: Cell<u32>,

    // ring_size: u32,
    fd: IoUringFd,
}

impl<'ring> Debug for SubmissionQueue<'ring> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SubmissionQueue")
            .field("head", &self.head.load(Relaxed))
            .field("tail", &self.tail.load(Relaxed))
            .field("mask", self.mask)
            .field("entries", &self.entries)
            // .field("flags", &self.flags.load(Relaxed))
            // .field("dropped", &self.dropped)
            // .field("array", &self.array)
            //.field("array_length", &self.array.len())
            .field("sqes", &self.sqes)
            // .field("sqe_head", &self.sqe_head)
            // .field("sqe_tail", &self.sqe_tail)
            .finish()
    }
}

impl<'ring> SubmissionQueue<'ring> {
    fn new(
        fd: IoUringFd,
        params: &io_uring_params,
        ring_ptr: NonNull<u8>,
        sqes: NonNull<SubmissionQueueEntry>,
    ) -> Self {
        let ring_ptr = ring_ptr.as_ptr();
        let sqes = sqes.as_ptr();

        unsafe {
            // let head = &*(ring_ptr.add(params.sq_off.head as usize) as *const AtomicU32);
            // let tail = &*(ring_ptr.add(params.sq_off.tail as usize) as *const AtomicU32);

            Self {
                // head,
                // tail,
                head: &*(ring_ptr.add(params.sq_off.head as usize) as *const AtomicU32),
                tail: &*(ring_ptr.add(params.sq_off.tail as usize) as *const AtomicU32),
                mask: &*(ring_ptr.add(params.sq_off.ring_mask as usize) as *const u32),
                entries: &*(ring_ptr.add(params.sq_off.ring_entries as usize) as *const u32),
                // flags: &*(ring_ptr.add(params.sq_off.flags as usize) as *const AtomicU32),
                // dropped: &*(ring_ptr.add(params.sq_off.dropped as usize) as *const u32),
                // array: std::slice::from_raw_parts(
                //     ring_ptr.add(params.sq_off.array as usize) as *const AtomicU32,
                //     params.sq_entries as usize,
                // ),
                sqes: std::slice::from_raw_parts(sqes as *mut _, params.sq_entries as usize),
                // sqe_head: Cell::new(head.load(Relaxed)),
                // sqe_tail: Cell::new(tail.load(Relaxed)),
                //ring_size,
                fd,
            }
        }
    }

    pub fn available(&self) -> usize {
        let tail = self.tail.load(Relaxed);
        let head = self.head.load(Acquire);

        (*self.entries - tail.wrapping_sub(head)) as usize
    }

    pub fn push_entry<'op>(&self, op: Operation<'op>) -> Result<(), Operation<'op>> {
        let tail = self.tail.load(Relaxed);

        if tail == self.head.load(Acquire).wrapping_add(*self.entries) {
            return Err(op);
        }

        let index = tail & *self.mask;
        let entry = unsafe { &mut *self.sqes[index as usize].get() };
        entry.apply_operation(op);

        self.tail.store(tail.wrapping_add(1), Release);

        Ok(())
    }

    pub fn push_entries<'op, F>(&self, mut fun: F) -> usize
    where
        F: FnMut() -> Option<Operation<'op>>,
    {
        let head = self.head.load(Acquire);
        let mut tail = self.tail.load(Relaxed);
        let mask = *self.mask;
        let end = head.wrapping_add(*self.entries);

        let mut index = 0;

        while tail != end {
            let entry = unsafe { &mut *self.sqes[(tail & mask) as usize].get() };

            match fun() {
                Some(op) => entry.apply_operation(op),
                _ => break,
            }

            index += 1;
            tail = tail.wrapping_add(1);
        }

        self.tail.store(tail, Release);

        index
    }

    pub fn submit(&self) -> std::io::Result<()> {
        let tail = self.tail.load(Relaxed);
        let head = self.head.load(Relaxed);

        // These might wrap around
        let submitted = tail.wrapping_sub(head);

        io_uring_enter(self.fd, submitted, 0, IORING_ENTER_GETEVENTS as u32)?;

        Ok(())
    }
}

#[derive(Debug)]
pub struct CompletionQueue<'ring> {
    head: &'ring AtomicU32,
    tail: &'ring AtomicU32,
    mask: &'ring u32,
    entries: &'ring u32,
    flags: &'ring u32,
    overflow: &'ring u32,

    cqes: &'ring [CompletionQueueEntry],

    fd: IoUringFd,
}

#[derive(Debug)]
pub struct Completed {
    pub user_data: u64,
    pub result: i32,
}

impl From<&CompletionQueueEntry> for Completed {
    fn from(e: &CompletionQueueEntry) -> Self {
        Self {
            user_data: e.user_data,
            result: e.res,
        }
    }
}

impl<'ring> CompletionQueue<'ring> {
    fn new(fd: IoUringFd, params: &io_uring_params, ring_ptr: NonNull<u8>) -> Self {
        let ring_ptr = ring_ptr.as_ptr();

        unsafe {
            Self {
                head: &*(ring_ptr.add(params.cq_off.head as usize) as *const AtomicU32),
                tail: &*(ring_ptr.add(params.cq_off.tail as usize) as *const AtomicU32),
                mask: &*(ring_ptr.add(params.cq_off.ring_mask as usize) as *const u32),
                entries: &*(ring_ptr.add(params.cq_off.ring_entries as usize) as *const u32),
                flags: &*(ring_ptr.add(params.cq_off.flags as usize) as *const u32),
                overflow: &*(ring_ptr.add(params.cq_off.overflow as usize) as *const u32),
                cqes: std::slice::from_raw_parts(
                    ring_ptr.add(params.cq_off.cqes as usize) as *const CompletionQueueEntry,
                    params.cq_entries as usize,
                ),
                fd,
            }
        }
    }

    pub fn pop(&self) -> Option<Completed> {
        let tail = self.tail.load(Acquire);
        let head = self.head.load(Relaxed);

        if tail == head {
            return None;
        }

        let index = head & *self.mask;
        let entry = (&self.cqes[index as usize]).into();

        // Atomic: The entry must be read before modifying the head
        self.head.store(head.wrapping_add(1), Release);

        Some(entry)
    }

    pub fn wait_pop(&self) -> std::io::Result<Completed> {
        if let Some(e) = self.pop() {
            return Ok(e);
        };

        io_uring_enter(self.fd, 0, 1, IORING_ENTER_GETEVENTS as u32)?;

        Ok(self.pop().unwrap())
    }

    pub fn pending(&self) -> usize {
        let tail = self.tail.load(Acquire);
        let head = self.head.load(Relaxed);

        tail.wrapping_sub(head) as usize
    }
}

#[derive(Debug)]
pub struct IoUring {
    fd: IoUringFd,
    sq_ptr: NonNull<u8>,
    cq_ptr: NonNull<u8>,
    sq_size: u32,
    cq_size: u32,
    sqes_ptr: NonNull<SubmissionQueueEntry>,
    sqes_size: u32,
    flags: u32,
    features: FeaturesFlags,
    params: io_uring_params,
}

unsafe impl Send for IoUring {}

impl IoUring {
    pub fn new(len: u32) -> std::io::Result<Self> {
        let mut params = unsafe { std::mem::zeroed() };
        let io_ring_fd = io_uring_setup(len, &mut params)?;

        display_io_uring_features(params.features);

        let features = FeaturesFlags::from_bits_truncate(params.features);

        let has_single_mmap = features.contains(FeaturesFlags::IORING_FEAT_SINGLE_MMAP);

        let mut sq_ring_size =
            params.sq_off.array + params.sq_entries * std::mem::size_of::<c_uint>() as u32;
        let mut cq_ring_size = params.cq_off.cqes
            + params.cq_entries * std::mem::size_of::<CompletionQueueEntry>() as u32;

        if has_single_mmap {
            if cq_ring_size > sq_ring_size {
                sq_ring_size = cq_ring_size;
            }
            cq_ring_size = sq_ring_size;
        }

        let sq_ring_ptr = mmap(&io_ring_fd, sq_ring_size, IORING_OFF_SQ_RING)?;

        unsafe {
            // Let's write `array` values now.
            // Each value is an index in `sqes`.
            // Our implementation won't modify them and the kernel too.
            // https://github.com/torvalds/linux/blob/71c5f03154ac1cb27423b984743ccc2f5d11d14d/fs/io_uring.c#L274
            let array_ptr = (sq_ring_ptr.as_ptr() as *mut u8).add(params.sq_off.array as usize);
            let array_ptr = array_ptr as *mut u32;

            for index in 0..params.sq_entries {
                array_ptr.add(index as usize).write(index);
            }
        }

        let cq_ring_ptr = if has_single_mmap {
            sq_ring_ptr
        } else {
            match mmap(&io_ring_fd, cq_ring_size, IORING_OFF_CQ_RING) {
                Ok(ptr) => ptr,
                Err(e) => {
                    munmap(sq_ring_ptr, sq_ring_size).unwrap();
                    return Err(e);
                }
            }
        };

        let sqes_size = params.sq_entries * std::mem::size_of::<CompletionQueueEntry>() as u32;
        let sqes = match mmap(&io_ring_fd, sqes_size, IORING_OFF_SQES) {
            Ok(ptr) => ptr,
            Err(e) => {
                munmap(sq_ring_ptr, sq_ring_size).unwrap();
                munmap(cq_ring_ptr, cq_ring_size).unwrap();
                return Err(e);
            }
        };

        Ok(IoUring {
            fd: io_ring_fd,
            sq_ptr: sq_ring_ptr,
            cq_ptr: cq_ring_ptr,
            sq_size: sq_ring_size,
            cq_size: cq_ring_size,
            sqes_ptr: sqes,
            flags: params.flags,
            sqes_size,
            features,
            params,
        })
    }

    pub fn sq(&self) -> SubmissionQueue {
        SubmissionQueue::new(self.fd, &self.params, self.sq_ptr, self.sqes_ptr)
    }

    pub fn cq(&self) -> CompletionQueue {
        CompletionQueue::new(self.fd, &self.params, self.cq_ptr)
    }
}

impl Drop for IoUring {
    fn drop(&mut self) {
        munmap(self.sqes_ptr, self.sqes_size).unwrap();
        munmap(self.sq_ptr, self.sq_size).unwrap();
        if self.sq_ptr != self.cq_ptr {
            munmap(self.cq_ptr, self.cq_size).unwrap();
        }
    }
}

#[cfg(test)]
mod tests {
    use std::iter::repeat_with;

    use super::{IoUring, Operation};

    #[test]
    #[cfg_attr(miri, ignore)] // Miri doesn't support io_uring
    fn simple() {
        let iou = IoUring::new(4).unwrap();

        let sq = iou.sq();

        sq.push_entry(Operation::NoOp).unwrap();
        sq.push_entry(Operation::NoOp).unwrap();
        sq.push_entry(Operation::NoOp).unwrap();
        sq.push_entry(Operation::NoOp).unwrap();
        sq.push_entry(Operation::NoOp).unwrap_err();

        sq.submit().unwrap();

        sq.push_entry(Operation::NoOp).unwrap();
        sq.push_entry(Operation::NoOp).unwrap();

        sq.submit().unwrap();

        sq.push_entry(Operation::NoOp).unwrap();
        sq.push_entry(Operation::NoOp).unwrap();
        sq.push_entry(Operation::NoOp).unwrap();
        sq.push_entry(Operation::NoOp).unwrap();
        sq.push_entry(Operation::NoOp).unwrap_err();
    }

    #[test]
    #[cfg_attr(miri, ignore)] // Miri doesn't support io_uring
    fn multiple() {
        let iou = IoUring::new(4).unwrap();

        let sq = iou.sq();

        let mut iter = repeat_with(|| Operation::NoOp).take(4);
        let n = sq.push_entries(|| iter.next());
        assert_eq!(n, 4);

        let n = sq.push_entries(|| Some(Operation::NoOp));
        assert_eq!(n, 0);
    }

    #[test]
    #[cfg_attr(miri, ignore)] // Miri doesn't support io_uring
    fn pop() {
        let iou = IoUring::new(4).unwrap();

        let sq = iou.sq();
        let cq = iou.cq();

        let mut iter = repeat_with(|| Operation::NoOp).take(4);
        let n = sq.push_entries(|| iter.next());
        assert_eq!(n, 4);
        sq.submit().unwrap();

        assert!(cq.pending() > 0);

        cq.pop().unwrap();
        cq.pop().unwrap();
        cq.pop().unwrap();
        cq.pop().unwrap();
        assert!(cq.pop().is_none());

        let mut spec = libc::timespec {
            tv_sec: 0,
            tv_nsec: 100000000, // 0,1 sec
        };

        sq.push_entry(Operation::TimeOut { spec: &mut spec })
            .unwrap();
        sq.submit().unwrap();

        assert!(cq.pop().is_none());

        let val = cq.wait_pop().unwrap();
        assert_eq!(val.result, -libc::ETIME);

        sq.push_entry(Operation::NoOp).unwrap();
        sq.push_entry(Operation::NoOp).unwrap();
        sq.submit().unwrap();
        sq.submit().unwrap();

        std::thread::sleep(std::time::Duration::from_millis(100));

        let val = cq.wait_pop().unwrap();
        let val = cq.wait_pop().unwrap();

        println!("val={:?}", val);
        println!("sq={:#?}", sq);
        println!("cq={:#?}", cq);
        println!("iou={:#?}", iou);
    }
}
