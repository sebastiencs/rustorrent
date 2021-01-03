use std::{fmt::Debug, os::unix::io::RawFd};

use bitflags::bitflags;
use libc::{c_int, c_long, c_uint, off_t};
use static_assertions::{assert_eq_size, assert_type_eq_all};

// Make sure our types size match the ones on the kernel
assert_eq_size!(io_uring_params, [u8; 120]);
assert_eq_size!(io_cqring_offsets, [u8; 40]);
assert_eq_size!(io_sqring_offsets, [u8; 40]);
assert_type_eq_all!(c_uint, u32);
assert_type_eq_all!(c_int, i32);
assert_eq_size!(SubmissionQueueEntry, [u8; 64]);
assert_eq_size!(CompletionQueueEntry, [u8; 16]);
assert_eq_size!(SqeFlags, u8);
assert_eq_size!(EnterFlags, u32);
assert_eq_size!(SqFlags, u32);
assert_eq_size!(timespec, u128);

pub(super) const SYS_IO_URING_SETUP: c_long = 425;
pub(super) const SYS_IO_URING_ENTER: c_long = 426;
pub(super) const SYS_IO_URING_REGISTER: c_long = 427;

// Magic offsets for the application to mmap the data it needs
pub(super) const IORING_OFF_SQ_RING: off_t = 0;
pub(super) const IORING_OFF_CQ_RING: off_t = 0x8000000;
pub(super) const IORING_OFF_SQES: off_t = 0x10000000;

pub(super) const IORING_OP_NOP: u8 = 0;
pub(super) const IORING_OP_READV: u8 = 1;
pub(super) const IORING_OP_WRITEV: u8 = 2;
pub(super) const IORING_OP_FSYNC: u8 = 3;
pub(super) const IORING_OP_READ_FIXED: u8 = 4;
pub(super) const IORING_OP_WRITE_FIXED: u8 = 5;
pub(super) const IORING_OP_POLL_ADD: u8 = 6;
pub(super) const IORING_OP_POLL_REMOVE: u8 = 7;
pub(super) const IORING_OP_SYNC_FILE_RANGE: u8 = 8;
pub(super) const IORING_OP_SENDMSG: u8 = 9;
pub(super) const IORING_OP_RECVMSG: u8 = 10;
pub(super) const IORING_OP_TIMEOUT: u8 = 11;
pub(super) const IORING_OP_TIMEOUT_REMOVE: u8 = 12;
pub(super) const IORING_OP_ACCEPT: u8 = 13;
pub(super) const IORING_OP_ASYNC_CANCEL: u8 = 14;
pub(super) const IORING_OP_LINK_TIMEOUT: u8 = 15;
pub(super) const IORING_OP_CONNECT: u8 = 16;
pub(super) const IORING_OP_FALLOCATE: u8 = 17;
pub(super) const IORING_OP_OPENAT: u8 = 18;
pub(super) const IORING_OP_CLOSE: u8 = 19;
pub(super) const IORING_OP_FILES_UPDATE: u8 = 20;
pub(super) const IORING_OP_STATX: u8 = 21;
pub(super) const IORING_OP_READ: u8 = 22;
pub(super) const IORING_OP_WRITE: u8 = 23;
pub(super) const IORING_OP_FADVISE: u8 = 24;
pub(super) const IORING_OP_MADVISE: u8 = 25;
pub(super) const IORING_OP_SEND: u8 = 26;
pub(super) const IORING_OP_RECV: u8 = 27;
pub(super) const IORING_OP_OPENAT2: u8 = 28;
pub(super) const IORING_OP_EPOLL_CTL: u8 = 29;
pub(super) const IORING_OP_SPLICE: u8 = 30;
pub(super) const IORING_OP_PROVIDE_BUFFERS: u8 = 31;
pub(super) const IORING_OP_REMOVE_BUFFERS: u8 = 32;
pub(super) const IORING_OP_TEE: u8 = 33;
pub(super) const IORING_OP_SHUTDOWN: u8 = 34;
pub(super) const IORING_OP_RENAMEAT: u8 = 35;
pub(super) const IORING_OP_UNLINKAT: u8 = 36;

pub(super) const IORING_REGISTER_BUFFERS: u32 = 0;
pub(super) const IORING_UNREGISTER_BUFFERS: u32 = 1;
pub(super) const IORING_REGISTER_FILES: u32 = 2;
pub(super) const IORING_UNREGISTER_FILES: u32 = 3;
pub(super) const IORING_REGISTER_EVENTFD: u32 = 4;
pub(super) const IORING_UNREGISTER_EVENTFD: u32 = 5;
pub(super) const IORING_REGISTER_FILES_UPDATE: u32 = 6;
pub(super) const IORING_REGISTER_EVENTFD_ASYNC: u32 = 7;
pub(super) const IORING_REGISTER_PROBE: u32 = 8;
pub(super) const IORING_REGISTER_PERSONALITY: u32 = 9;
pub(super) const IORING_UNREGISTER_PERSONALITY: u32 = 10;
pub(super) const IORING_REGISTER_RESTRICTIONS: u32 = 11;
pub(super) const IORING_REGISTER_ENABLE_RINGS: u32 = 12;

bitflags! {
    pub(super) struct EnterFlags: u32 {
        const IORING_ENTER_GETEVENTS = 1 << 0;
        const IORING_ENTER_SQ_WAKEUP = 1 << 1;
        const IORING_ENTER_SQ_WAIT = 1 << 2;
        const IORING_ENTER_EXT_ARG = 1 << 3;
    }
}

bitflags! {
    // sqe->sqe_flags
    #[derive(Default)]
    pub(super) struct SqeFlags: u8 {
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

bitflags! {
    pub(super) struct FeaturesFlags: u32 {
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
    pub(super) struct SqFlags: u32 {
        /// needs io_uring_enter wakeup
        const IORING_SQ_NEED_WAKEUP = 1 << 0;
        /// CQ ring is overflown
        const IORING_SQ_CQ_OVERFLOW = 1 << 1;
    }
}

#[derive(Debug, Clone)]
#[repr(C)]
pub(super) struct CompletionQueueEntry {
    pub(super) user_data: u64,
    pub(super) res: i32,
    pub(super) flags: u32,
}

#[allow(non_camel_case_types)]
#[derive(Debug, Clone, Default)]
#[repr(C)]
pub(super) struct io_sqring_offsets {
    pub(super) head: u32,
    pub(super) tail: u32,
    pub(super) ring_mask: u32,
    pub(super) ring_entries: u32,
    pub(super) flags: u32,
    pub(super) dropped: u32,
    pub(super) array: u32,
    pub(super) resv: [u32; 3],
}

#[allow(non_camel_case_types)]
#[derive(Debug, Clone, Default)]
#[repr(C)]
pub(super) struct io_cqring_offsets {
    pub(super) head: u32,
    pub(super) tail: u32,
    pub(super) ring_mask: u32,
    pub(super) ring_entries: u32,
    pub(super) overflow: u32,
    pub(super) cqes: u32,
    pub(super) flags: u32,
    pub(super) resv: [u32; 3],
}

#[allow(non_camel_case_types)]
#[derive(Debug, Clone, Default)]
#[repr(C)]
pub(super) struct io_uring_params {
    pub(super) sq_entries: u32,
    pub(super) cq_entries: u32,
    pub(super) flags: u32,
    pub(super) sq_thread_cpu: u32,
    pub(super) sq_thread_idle: u32,
    pub(super) features: u32,
    pub(super) resv: [u32; 4],
    pub(super) sq_off: io_sqring_offsets,
    pub(super) cq_off: io_cqring_offsets,
}

#[allow(non_camel_case_types)]
#[derive(Debug, Clone)]
#[repr(C)]
pub(super) struct io_uring_files_update {
    pub(super) offset: u32,
    pub(super) resv: u32,
    pub(super) fds: u64,
}

#[allow(non_camel_case_types)]
#[derive(Debug, Clone)]
#[repr(C)]
pub(super) struct io_uring_probe_op {
    op: u8,
    resv: u8,
    flags: u16,
    resv2: u32,
}

#[allow(non_camel_case_types)]
#[derive(Debug, Clone, Default)]
#[repr(C)]
pub(super) struct io_uring_probe {
    pub(super) last_op: u8,
    pub(super) ops_len: u8,
    pub(super) resv: u16,
    pub(super) resv2: [u32; 3],
    pub(super) ops: [io_uring_probe_op; 0],
}

#[derive(Debug, Copy, Clone)]
pub(super) struct IoUringFd(pub(super) RawFd);

pub(super) type SubmissionFlags = u32;

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

#[derive(Debug, Default)]
#[repr(C)]
pub(super) struct SubmissionQueueEntry {
    /// type of operation for this sqe
    pub(super) opcode: u8,
    /// IOSQE_ flags
    pub(super) sqe_flags: SqeFlags,
    /// ioprio for the request
    pub(super) ioprio: u16,
    /// file descriptor to do IO on
    pub(super) fd: i32,
    /// off: offset into file
    pub(super) off_addr2: u64,
    /// pointer to buffer or iovecs
    pub(super) addr_splice_off_in: u64,
    pub(super) len: u32,
    pub(super) flags: SubmissionFlags,
    pub(super) user_data: u64,
    pub(super) buf_index: u16,
    pub(super) personality: u16,
    pub(super) splice_fd_in: u32,
    pub(super) _pad: [u64; 2],
}

#[derive(Debug)]
#[allow(non_camel_case_types)]
#[repr(C)]
pub struct timespec {
    pub(super) tv_sec: i64,
    pub(super) tv_nsec: libc::c_longlong,
}
