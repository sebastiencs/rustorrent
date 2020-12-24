use std::{os::unix::io::RawFd, ptr::NonNull, sync::atomic::AtomicU32};

use bitflags::bitflags;
use libc::{c_int, c_long, c_uint, c_void, off_t, syscall};
use static_assertions::{assert_eq_size, assert_type_eq_all};

const SYS_IO_URING_SETUP: c_long = 425;
const SYS_IO_URING_ENTER: c_long = 426;
const SYS_IO_URING_REGISTER: c_long = 427;

// Magic offsets for the application to mmap the data it needs
const IORING_OFF_SQ_RING: off_t = 0;
const IORING_OFF_CQ_RING: off_t = 0x8000000;
const IORING_OFF_SQES: off_t = 0x10000000;

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
struct CompletetionQueueEvent {
    user_data: u64,
    res: i32,
    flags: u32,
}

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

#[derive(Debug)]
pub(super) struct IoUringFd(RawFd);

pub(super) fn io_uring_setup(
    entries: u32,
    params: &mut io_uring_params,
) -> std::io::Result<IoUringFd> {
    let res = unsafe {
        // int io_uring_setup(u32 entries, struct io_uring_params *p);
        syscall(SYS_IO_URING_SETUP, entries, params as *mut io_uring_params)
    };

    if res < 0 {
        return Err(std::io::Error::last_os_error());
    }

    Ok(IoUringFd(res as RawFd))
}

pub(super) fn io_uring_enter(fd: i32, to_submit: u32, min_complete: u32, flags: u32) -> isize {
    unsafe {
        // int io_uring_enter(unsigned int fd, unsigned int to_submit,
        //                    unsigned int min_complete, unsigned int flags,
        //                    sigset_t *sig);
        syscall(
            SYS_IO_URING_ENTER,
            fd,
            to_submit,
            min_complete,
            flags,
            std::ptr::null_mut::<()>(),
        ) as isize
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

#[derive(Debug)]
struct SubmissionQueueEvent {}

#[derive(Debug)]
struct SubmissionQueue<'ring> {
    head: &'ring AtomicU32,
    tail: &'ring AtomicU32,
    mask: &'ring u32,
    entries: &'ring u32,
    flags: &'ring u32,
    dropped: &'ring u32,
    array: &'ring [u32],

    sqes: &'ring [SubmissionQueueEvent],
    sqe_head: u32,
    sqe_tail: u32,

    ring_size: u32,
    // ring: *const (),
}

impl<'ring> SubmissionQueue<'ring> {
    fn new(
        params: &io_uring_params,
        ring_ptr: NonNull<u8>,
        ring_size: u32,
        sqes: NonNull<SubmissionQueueEvent>,
    ) -> Self {
        let ring_ptr = ring_ptr.as_ptr();
        let sqes = sqes.as_ptr();

        unsafe {
            Self {
                head: &*(ring_ptr.add(params.sq_off.head as usize) as *const AtomicU32),
                tail: &*(ring_ptr.add(params.sq_off.tail as usize) as *const AtomicU32),
                mask: &*(ring_ptr.add(params.sq_off.ring_mask as usize) as *const u32),
                entries: &*(ring_ptr.add(params.sq_off.ring_entries as usize) as *const u32),
                flags: &*(ring_ptr.add(params.sq_off.flags as usize) as *const u32),
                dropped: &*(ring_ptr.add(params.sq_off.dropped as usize) as *const u32),
                array: std::slice::from_raw_parts(
                    ring_ptr.add(params.sq_off.array as usize) as *const u32,
                    params.sq_entries as usize,
                ),
                sqes: std::slice::from_raw_parts(sqes, params.sq_entries as usize),
                sqe_head: 0,
                sqe_tail: 0,
                ring_size,
            }
        }
    }
}

#[derive(Debug)]
struct CompletionQueue<'ring> {
    head: &'ring AtomicU32,
    tail: &'ring AtomicU32,
    mask: &'ring u32,
    entries: &'ring u32,
    flags: &'ring u32,
    overflow: &'ring u32,

    cqes: &'ring [CompletetionQueueEvent],

    ring_size: u32,
}

impl<'ring> CompletionQueue<'ring> {
    fn new(params: &io_uring_params, ring_ptr: NonNull<u8>, ring_size: u32) -> Self {
        let ring_ptr = ring_ptr.as_ptr();

        unsafe {
            Self {
                head: &*(ring_ptr.add(params.cq_off.head as usize) as *const u32
                    as *const AtomicU32),
                tail: &*(ring_ptr.add(params.cq_off.tail as usize) as *const AtomicU32),
                mask: &*(ring_ptr.add(params.cq_off.ring_mask as usize) as *const u32),
                entries: &*(ring_ptr.add(params.cq_off.ring_entries as usize) as *const u32),
                flags: &*(ring_ptr.add(params.cq_off.flags as usize) as *const u32),
                overflow: &*(ring_ptr.add(params.cq_off.overflow as usize) as *const u32),
                cqes: std::slice::from_raw_parts(
                    ring_ptr.add(params.cq_off.cqes as usize) as *const CompletetionQueueEvent,
                    params.cq_entries as usize,
                ),
                ring_size,
            }
        }
    }
}

#[derive(Debug)]
pub struct IoUring {
    fd: IoUringFd,
    sq_ptr: NonNull<u8>,
    cq_ptr: NonNull<u8>,
    sq_size: u32,
    cq_size: u32,
    sqes_ptr: NonNull<SubmissionQueueEvent>,
    sqes_size: u32,
    flags: u32,
    features: FeaturesFlags,
    params: io_uring_params,
}

impl IoUring {
    #[allow(clippy::clippy::new_ret_no_self)]
    pub fn new() {
        let mut params = unsafe { std::mem::zeroed() };
        let io_ring_fd = io_uring_setup(256, &mut params).unwrap();

        println!("PARAMS {:#?}", params);
        display_io_uring_features(params.features);

        let features = FeaturesFlags::from_bits_truncate(params.features);

        let has_single_mmap = features.contains(FeaturesFlags::IORING_FEAT_SINGLE_MMAP);

        let mut sq_ring_size =
            params.sq_off.array + params.sq_entries * std::mem::size_of::<c_uint>() as u32;
        let mut cq_ring_size = params.cq_off.cqes
            + params.cq_entries * std::mem::size_of::<CompletetionQueueEvent>() as u32;

        if has_single_mmap {
            if cq_ring_size > sq_ring_size {
                sq_ring_size = cq_ring_size;
            }
            cq_ring_size = sq_ring_size;
        }

        let sq_ring_ptr = mmap(&io_ring_fd, sq_ring_size, IORING_OFF_SQ_RING).unwrap();

        let cq_ring_ptr = if has_single_mmap {
            sq_ring_ptr
        } else {
            mmap(&io_ring_fd, cq_ring_size, IORING_OFF_CQ_RING).unwrap()
        };

        let sqes_size = params.sq_entries * std::mem::size_of::<CompletetionQueueEvent>() as u32;
        let sqes = mmap(&io_ring_fd, sqes_size, IORING_OFF_SQES).unwrap();

        IoUring {
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
        };
    }

    fn sq(&self) -> SubmissionQueue {
        SubmissionQueue::new(&self.params, self.sq_ptr, self.sq_size, self.sqes_ptr)
    }

    fn cq(&self) -> CompletionQueue {
        CompletionQueue::new(&self.params, self.cq_ptr, self.cq_size)
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
