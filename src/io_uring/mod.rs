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

use self::file::{FileDescriptor, RingId};

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

const IORING_REGISTER_BUFFERS: u32 = 0;
const IORING_UNREGISTER_BUFFERS: u32 = 1;
const IORING_REGISTER_FILES: u32 = 2;
const IORING_UNREGISTER_FILES: u32 = 3;
const IORING_REGISTER_EVENTFD: u32 = 4;
const IORING_UNREGISTER_EVENTFD: u32 = 5;
const IORING_REGISTER_FILES_UPDATE: u32 = 6;
const IORING_REGISTER_EVENTFD_ASYNC: u32 = 7;
const IORING_REGISTER_PROBE: u32 = 8;
const IORING_REGISTER_PERSONALITY: u32 = 9;
const IORING_UNREGISTER_PERSONALITY: u32 = 10;
const IORING_REGISTER_RESTRICTIONS: u32 = 11;
const IORING_REGISTER_ENABLE_RINGS: u32 = 12;

bitflags! {
    struct EnterFlags: u32 {
        const IORING_ENTER_GETEVENTS = 1 << 0;
        const IORING_ENTER_SQ_WAKEUP = 1 << 1;
        const IORING_ENTER_SQ_WAIT = 1 << 2;
        const IORING_ENTER_EXT_ARG = 1 << 3;
    }
}

bitflags! {
    // sqe->sqe_flags
    struct SqeFlags: u8 {
        // use fixed fileset
        const IOSQE_FIXED_FILE = 1 << 0;
        // issue after inflight IO
        const IOSQE_IO_DRAIN = 1 << 1;
        // links next sqe
        const IOSQE_IO_LINK = 1 << 2;
        // like LINK, but stronger
        const IOSQE_IO_HARDLINK = 1 << 3;
        // always go async
        const IOSQE_ASYNC = 1 << 4;
        // select buffer from sqe->buf_group
        const IOSQE_BUFFER_SELECT = 1 << 5;
    }
}

assert_eq_size!(SqeFlags, u8);

bitflags! {
    pub struct FeaturesFlags: u32 {
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

bitflags! {
    /// sq_ring->flags
    pub struct SqFlags: u32 {
        /// needs io_uring_enter wakeup
        const IORING_SQ_NEED_WAKEUP = 1 << 0;
        /// CQ ring is overflown
        const IORING_SQ_CQ_OVERFLOW = 1 << 1;
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
#[repr(C)]
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
#[repr(C)]
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
#[repr(C)]
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

#[allow(non_camel_case_types)]
#[derive(Debug, Clone)]
#[repr(C)]
pub(super) struct io_uring_files_update {
    offset: u32,
    resv: u32,
    fds: u64,
}

assert_eq_size!(io_uring_params, [u8; 120]);
assert_eq_size!(io_cqring_offsets, [u8; 40]);
assert_eq_size!(io_sqring_offsets, [u8; 40]);
assert_type_eq_all!(c_uint, u32);
assert_type_eq_all!(c_int, i32);

#[derive(Debug, Copy, Clone)]
pub struct IoUringFd(RawFd);

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

pub(super) fn io_uring_register(
    fd: IoUringFd,
    opcode: u32,
    arg: *const (),
    nr_args: u32,
) -> std::io::Result<()> {
    let res = unsafe {
        // int io_uring_register(unsigned int fd, unsigned int opcode,
        //                       void *arg, unsigned int nr_args);
        syscall(SYS_IO_URING_REGISTER, fd.0, opcode, arg, nr_args) as isize
    };

    if res < 0 {
        return Err(std::io::Error::last_os_error());
    }

    Ok(())
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

type SubmissionFlags = u32;

// union SubmissionFlags {
//     rw_flags: RWFlags,
//     fsync_flags: u32,
//     poll_events: u16,
//     poll32_events: u32,
//     sync_range_flags: u32,
//     msg_flags: u32,
//     timeout_flags: u32,
//     accept_flags: u32,
//     cancel_flags: u32,
//     open_flags: u32,
//     statx_flags: u32,
//     fadvise_advice: u32,
//     splice_flags: u32,
//     rename_flags: u32,
//     unlink_flags: u32,
// }

// impl Debug for SubmissionFlags {
//     fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
//         unsafe { write!(f, "SubmissionFlags {:032b}", self.fsync_flags) }
//     }
// }

#[derive(Debug)]
#[repr(C)]
struct SubmissionQueueEntry {
    /// type of operation for this sqe
    opcode: u8,
    /// IOSQE_ flags
    sqe_flags: SqeFlags,
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
    RegisterFiles {
        id: RingId,
        io_uring_fd: IoUringFd,
        files: &'a [i32],
        link_next: bool,
    },
    Read {
        id: RingId,
        fd: FileDescriptor,
        offset: usize,
        data: &'a mut [u8],
    },
    Write {
        id: RingId,
        fd: FileDescriptor,
        offset: usize,
        data: &'a [u8],
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
            Operation::RegisterFiles {
                id,
                io_uring_fd,
                files,
                link_next,
            } => {
                self.opcode = IORING_OP_FILES_UPDATE;
                self.fd = -1;
                self.addr_splice_off_in = files.as_ptr() as u64;
                self.off_addr2 = 0;
                self.len = files.len() as u32;
                self.user_data = id.into();
                self.sqe_flags.set(SqeFlags::IOSQE_IO_LINK, link_next);
                //self.flags.rw_flags.set(RWFlags::IOSQE_IO_LINK, link_next);
            }
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
                id,
                fd,
                offset,
                data,
            } => {
                self.opcode = IORING_OP_READ;
                self.fd = match fd {
                    FileDescriptor::RegisteredIndex(index) => {
                        self.sqe_flags.set(SqeFlags::IOSQE_FIXED_FILE, true);
                        index
                    }
                    FileDescriptor::Fd(fd) => fd,
                };
                self.off_addr2 = offset as u64;
                self.addr_splice_off_in = data.as_ptr() as u64;
                self.len = data.len() as u32;
                self.user_data = id.into();
            }
            Operation::Write {
                id,
                fd,
                offset,
                data,
            } => {
                self.opcode = IORING_OP_WRITE;
                self.fd = match fd {
                    FileDescriptor::RegisteredIndex(index) => {
                        self.sqe_flags.set(SqeFlags::IOSQE_FIXED_FILE, true);
                        index
                    }
                    FileDescriptor::Fd(fd) => fd,
                };
                self.addr_splice_off_in = data.as_ptr() as u64;
                self.off_addr2 = offset as u64;
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
                self.flags = flags as u32;
                //self.flags.open_flags = flags as u32;
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

pub enum Submit {
    All,
    None,
    N(u32),
}

impl From<u32> for Submit {
    fn from(n: u32) -> Self {
        Submit::N(n)
    }
}

#[derive(Debug)]
pub struct IoUring {
    fd: IoUringFd,
    params: io_uring_params,

    sq_ptr: NonNull<u8>,
    cq_ptr: NonNull<u8>,
    sq_size: u32,
    cq_size: u32,
    sqes_size: u32,

    sq_entries: u32,
    sq_mask: u32,
    sq_head: NonNull<AtomicU32>,
    sq_tail: NonNull<AtomicU32>,
    sqes_ptr: NonNull<SubmissionQueueEntry>,

    cq_entries: u32,
    cq_mask: u32,
    cq_head: NonNull<AtomicU32>,
    cq_tail: NonNull<AtomicU32>,
    cqes_ptr: NonNull<CompletionQueueEntry>,
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

        let (sq_head, sq_tail, sq_entries, sq_mask) = unsafe {
            let sq_ptr = sq_ring_ptr.as_ptr() as *const u8;
            let head = sq_ptr.add(params.sq_off.head as usize) as *mut AtomicU32;
            let tail = sq_ptr.add(params.sq_off.tail as usize) as *mut AtomicU32;
            let entries = *(sq_ptr.add(params.sq_off.ring_entries as usize) as *const u32);
            let mask = *(sq_ptr.add(params.sq_off.ring_mask as usize) as *const u32);

            (head, tail, entries, mask)
        };

        let (cq_head, cq_tail, cq_entries, cq_mask, cqes_ptr) = unsafe {
            let cq_ptr = cq_ring_ptr.as_ptr() as *const u8;
            let head = cq_ptr.add(params.cq_off.head as usize) as *mut AtomicU32;
            let tail = cq_ptr.add(params.cq_off.tail as usize) as *mut AtomicU32;
            let entries = *(cq_ptr.add(params.cq_off.ring_entries as usize) as *const u32);
            let mask = *(cq_ptr.add(params.cq_off.ring_mask as usize) as *const u32);
            let cqes_ptr = cq_ptr.add(params.cq_off.cqes as usize) as *mut CompletionQueueEntry;

            (head, tail, entries, mask, cqes_ptr)
        };

        Ok(IoUring {
            fd: io_ring_fd,
            sq_ptr: sq_ring_ptr,
            cq_ptr: cq_ring_ptr,
            sq_size: sq_ring_size,
            cq_size: cq_ring_size,
            sqes_ptr: sqes,
            sqes_size,
            params,
            sq_mask,
            sq_entries,
            sq_tail: NonNull::new(sq_tail).unwrap(),
            sq_head: NonNull::new(sq_head).unwrap(),
            cq_entries,
            cq_mask,
            cq_tail: NonNull::new(cq_tail).unwrap(),
            cq_head: NonNull::new(cq_head).unwrap(),
            cqes_ptr: NonNull::new(cqes_ptr).unwrap(),
        })
    }

    pub fn features(&self) -> FeaturesFlags {
        FeaturesFlags::from_bits_truncate(self.params.features)
    }

    pub fn register_files(&self, files: &[i32]) -> std::io::Result<()> {
        io_uring_register(
            self.fd,
            IORING_REGISTER_FILES,
            files.as_ptr() as *mut _,
            files.len() as u32,
        )
    }

    /// Update a single entry
    pub fn register_file_update(&self, files: &[i32], offset: usize) -> std::io::Result<()> {
        let update = io_uring_files_update {
            offset: offset as u32,
            resv: 0,
            fds: files.as_ptr() as u64,
        };
        io_uring_register(
            self.fd,
            IORING_REGISTER_FILES_UPDATE,
            &update as *const _ as *mut _,
            1,
        )
    }

    pub fn register_files_update(&self, files: &[i32]) -> std::io::Result<()> {
        let update = io_uring_files_update {
            offset: 0,
            resv: 0,
            fds: files.as_ptr() as u64,
        };
        io_uring_register(
            self.fd,
            IORING_REGISTER_FILES_UPDATE,
            &update as *const _ as *mut _,
            files.len() as u32,
        )
    }

    pub fn unregister_files(&self) -> std::io::Result<()> {
        io_uring_register(self.fd, IORING_UNREGISTER_FILES, std::ptr::null_mut(), 0)
    }

    fn sq_refs(&self) -> (&AtomicU32, &AtomicU32) {
        unsafe { (&*self.sq_head.as_ptr(), &*self.sq_tail.as_ptr()) }
    }

    fn cq_refs(&self) -> (&AtomicU32, &AtomicU32) {
        unsafe { (&*self.cq_head.as_ptr(), &*self.cq_tail.as_ptr()) }
    }

    /// Number of entries pushed by us, waiting to be read by the kernel
    pub fn sq_pending(&self) -> u32 {
        let (head_ref, tail_ref) = self.sq_refs();

        let head = head_ref.load(Acquire);
        let tail = tail_ref.load(Relaxed);

        tail.wrapping_sub(head)
    }

    pub fn push_entry<'op>(&self, op: Operation<'op>) -> Result<(), Operation<'op>> {
        let (head_ref, tail_ref) = self.sq_refs();

        let head = head_ref.load(Acquire);
        let tail = tail_ref.load(Relaxed);

        if tail == head.wrapping_add(self.sq_entries) {
            return Err(op);
        }

        let index = tail & self.sq_mask;
        assert!(index < self.sq_entries);

        let entry = unsafe {
            let sqes = self.sqes_ptr.as_ptr() as *mut SubmissionQueueEntry;
            &mut *sqes.add(index as usize)
        };
        entry.apply_operation(op);

        tail_ref.store(tail.wrapping_add(1), Release);

        Ok(())
    }

    pub fn push_entries<'op, F>(&self, mut fun: F) -> usize
    where
        F: FnMut() -> Option<Operation<'op>>,
    {
        let (head_ref, tail_ref) = self.sq_refs();

        let head = head_ref.load(Acquire);
        let mut tail = tail_ref.load(Relaxed);
        let mask = self.sq_mask;
        let end = head.wrapping_add(self.sq_entries);

        let mut index = 0;

        while tail != end {
            assert!((tail & mask) < self.sq_entries);

            let entry = unsafe {
                let sqes = self.sqes_ptr.as_ptr() as *mut SubmissionQueueEntry;
                &mut *sqes.add((tail & mask) as usize)
            };

            match fun() {
                Some(op) => entry.apply_operation(op),
                _ => break,
            }

            index += 1;
            tail = tail.wrapping_add(1);
        }

        tail_ref.store(tail, Release);

        index
    }

    pub fn submit<S: Into<Submit>>(&self, n: S) -> std::io::Result<()> {
        let (head_ref, tail_ref) = self.sq_refs();
        let submit = n.into();

        let head = head_ref.load(Acquire);
        let tail = tail_ref.load(Relaxed);

        // These might wrap around
        let pending = tail.wrapping_sub(head);

        let to_submit = match submit {
            Submit::All => pending,
            Submit::N(n) => n.min(pending),
            Submit::None => 0,
        };

        if to_submit > 0 {
            io_uring_enter(self.fd, to_submit, 0, 0)?;
        }

        Ok(())
    }

    pub fn pop(&self) -> Option<Completed> {
        let (head_ref, tail_ref) = self.cq_refs();

        let tail = tail_ref.load(Acquire);
        let head = head_ref.load(Relaxed);

        if tail == head {
            return None;
        }

        let index = head & self.cq_mask;
        assert!(index < self.cq_entries);

        let entry = unsafe {
            let cqes = self.cqes_ptr.as_ptr() as *const CompletionQueueEntry;
            Completed::from(&*cqes.add(index as usize))
        };

        // Atomic: The entry must be read before modifying the head
        head_ref.store(head.wrapping_add(1), Release);

        Some(entry)
    }

    pub fn wait_pop(&self) -> std::io::Result<Completed> {
        loop {
            if let Some(e) = self.pop() {
                return Ok(e);
            };

            // Submit at the same time
            let sq_pending = self.sq_pending();
            io_uring_enter(
                self.fd,
                sq_pending,
                1,
                EnterFlags::IORING_ENTER_GETEVENTS.bits,
            )?;
        }
    }

    /// Number of entries pushed by the kernel, but not yet read by us
    pub fn cq_pending(&self) -> u32 {
        let (head_ref, tail_ref) = self.cq_refs();

        let tail = tail_ref.load(Acquire);
        let head = head_ref.load(Relaxed);

        tail.wrapping_sub(head) as u32
    }
}

impl Drop for IoUring {
    fn drop(&mut self) {
        // TODO: Make sure the completion queue is empty before unmapping
        // Or the kernel will work on unmapped memory
        munmap(self.sqes_ptr, self.sqes_size).unwrap();
        munmap(self.sq_ptr, self.sq_size).unwrap();
        if self.sq_ptr != self.cq_ptr {
            munmap(self.cq_ptr, self.cq_size).unwrap();
        }
    }
}

#[cfg(test)]
mod tests {
    use std::{iter::repeat_with, os::unix::io::AsRawFd};

    use super::{IoUring, Operation, Submit};

    #[test]
    #[cfg_attr(miri, ignore)] // Miri doesn't support io_uring
    fn simple() {
        let iou = IoUring::new(4).unwrap();

        iou.push_entry(Operation::NoOp).unwrap();
        iou.push_entry(Operation::NoOp).unwrap();
        iou.push_entry(Operation::NoOp).unwrap();
        iou.push_entry(Operation::NoOp).unwrap();
        iou.push_entry(Operation::NoOp).unwrap_err();

        iou.submit(Submit::All).unwrap();

        iou.push_entry(Operation::NoOp).unwrap();
        iou.push_entry(Operation::NoOp).unwrap();

        iou.submit(Submit::All).unwrap();

        iou.push_entry(Operation::NoOp).unwrap();
        iou.push_entry(Operation::NoOp).unwrap();
        iou.push_entry(Operation::NoOp).unwrap();
        iou.push_entry(Operation::NoOp).unwrap();
        iou.push_entry(Operation::NoOp).unwrap_err();
    }

    #[test]
    #[cfg_attr(miri, ignore)] // Miri doesn't support io_uring
    fn multiple() {
        let iou = IoUring::new(4).unwrap();

        let mut iter = repeat_with(|| Operation::NoOp).take(4);
        let n = iou.push_entries(|| iter.next());
        assert_eq!(n, 4);

        let n = iou.push_entries(|| Some(Operation::NoOp));
        assert_eq!(n, 0);
    }

    #[test]
    #[cfg_attr(miri, ignore)] // Miri doesn't support io_uring
    fn pop() {
        let iou = IoUring::new(4).unwrap();

        let mut iter = repeat_with(|| Operation::NoOp).take(4);
        let n = iou.push_entries(|| iter.next());
        assert_eq!(n, 4);
        iou.submit(Submit::All).unwrap();

        assert!(iou.cq_pending() > 0);

        iou.pop().unwrap();
        iou.pop().unwrap();
        iou.pop().unwrap();
        iou.pop().unwrap();
        assert!(iou.pop().is_none());

        let mut spec = libc::timespec {
            tv_sec: 0,
            tv_nsec: 100000000, // 0,1 sec
        };

        iou.push_entry(Operation::TimeOut { spec: &mut spec })
            .unwrap();
        iou.submit(Submit::All).unwrap();

        assert!(iou.pop().is_none());

        let val = iou.wait_pop().unwrap();
        assert_eq!(val.result, -libc::ETIME);

        iou.push_entry(Operation::NoOp).unwrap();
        iou.push_entry(Operation::NoOp).unwrap();
        iou.submit(Submit::All).unwrap();
        iou.submit(Submit::All).unwrap();

        std::thread::sleep(std::time::Duration::from_millis(100));

        let val = iou.wait_pop().unwrap();
        let val = iou.wait_pop().unwrap();

        println!("val={:?}", val);
        println!("iou={:#?}", iou);
    }

    #[test]
    #[cfg_attr(miri, ignore)] // Miri doesn't support io_uring
    fn register_files() {
        let iou = IoUring::new(4).unwrap();

        let fd = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .read(true)
            .open("register.txt")
            .unwrap();

        let registered_files = vec![-1; 512];
        iou.register_files(&registered_files).unwrap();

        let mut files = Vec::new();
        files.push(fd.as_raw_fd());

        iou.push_entry(Operation::RegisterFiles {
            id: 101.into(),
            io_uring_fd: iou.fd,
            files: files.as_slice(),
            link_next: false,
        })
        .unwrap();
        iou.submit(Submit::All);

        let completed = iou.wait_pop().unwrap();

        assert_eq!(completed.result, 1);

        println!("completed={:?}", completed);
    }
}
