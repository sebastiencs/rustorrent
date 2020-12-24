use std::sync::atomic::AtomicU32;

use libc::{c_int, c_long, c_uint, syscall};
use static_assertions::{assert_eq_size, assert_type_eq_all};

const SYS_IO_URING_SETUP: i64 = 425;
const SYS_IO_URING_ENTER: i64 = 426;
const SYS_IO_URING_REGISTER: i64 = 427;

// Magic offsets for the application to mmap the data it needs
const IORING_OFF_SQ_RING: i64 = 0;
const IORING_OFF_CQ_RING: i64 = 0x8000000;
const IORING_OFF_SQES: i64 = 0x10000000;

const IORING_FEAT_SINGLE_MMAP: u32 = 1 << 0;
const IORING_FEAT_NODROP: u32 = 1 << 1;
const IORING_FEAT_SUBMIT_STABLE: u32 = 1 << 2;
const IORING_FEAT_RW_CUR_POS: u32 = 1 << 3;
const IORING_FEAT_CUR_PERSONALITY: u32 = 1 << 4;
const IORING_FEAT_FAST_POLL: u32 = 1 << 5;
const IORING_FEAT_POLL_32BITS: u32 = 1 << 6;
const IORING_FEAT_SQPOLL_NONFIXED: u32 = 1 << 7;
const IORING_FEAT_EXT_ARG: u32 = 1 << 8;

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
assert_type_eq_all!(c_long, i64);

pub(super) fn io_uring_setup(
    entries: u32,
    params: &mut io_uring_params,
) -> std::io::Result<IoUring> {
    let res = unsafe {
        // int io_uring_setup(u32 entries, struct io_uring_params *p);
        syscall(SYS_IO_URING_SETUP, entries, params as *mut io_uring_params) as i32
    };

    if res < 0 {
        return Err(std::io::Error::last_os_error());
    }

    Ok(IoUring { fd: res })
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

pub(super) fn mmap(io_uring: &IoUring, length: u32) -> std::io::Result<*mut libc::c_void> {
    let ptr = unsafe {
        // ptr = mmap(0, sq_off.array + sq_entries * sizeof(__u32),
        //            PROT_READ|PROT_WRITE, MAP_SHARED|MAP_POPULATE,
        //            ring_fd, IORING_OFF_SQ_RING);
        libc::mmap(
            std::ptr::null_mut(),
            length as usize,
            libc::PROT_READ | libc::PROT_WRITE,
            libc::MAP_SHARED | libc::MAP_POPULATE,
            io_uring.fd,
            IORING_OFF_SQ_RING,
        )
    };

    if ptr == libc::MAP_FAILED {
        return Err(std::io::Error::last_os_error());
    }

    Ok(ptr)
}

// const IORING_FEAT_SINGLE_MMAP: u32 = 1 << 0;
// const IORING_FEAT_NODROP: u32 = 1 << 1;
// const IORING_FEAT_SUBMIT_STABLE: u32 = 1 << 2;
// const IORING_FEAT_RW_CUR_POS: u32 = 1 << 3;
// const IORING_FEAT_CUR_PERSONALITY: u32 = 1 << 4;
// const IORING_FEAT_FAST_POLL: u32 = 1 << 5;
// const IORING_FEAT_POLL_32BITS: u32 = 1 << 6;
// const IORING_FEAT_SQPOLL_NONFIXED: u32 = 1 << 7;
// const IORING_FEAT_EXT_ARG: u32 = 1 << 8;

fn display_io_uring_features(features: u32) {
    let mut feats = Vec::with_capacity(9);

    if features & IORING_FEAT_SINGLE_MMAP != 0 {
        feats.push("IORING_FEAT_SINGLE_MMAP");
    }
    if features & IORING_FEAT_NODROP != 0 {
        feats.push("IORING_FEAT_NODROP");
    }
    if features & IORING_FEAT_SUBMIT_STABLE != 0 {
        feats.push("IORING_FEAT_SUBMIT_STABLE");
    }
    if features & IORING_FEAT_RW_CUR_POS != 0 {
        feats.push("IORING_FEAT_RW_CUR_POS");
    }
    if features & IORING_FEAT_CUR_PERSONALITY != 0 {
        feats.push("IORING_FEAT_CUR_PERSONALITY");
    }
    if features & IORING_FEAT_FAST_POLL != 0 {
        feats.push("IORING_FEAT_FAST_POLL");
    }
    if features & IORING_FEAT_POLL_32BITS != 0 {
        feats.push("IORING_FEAT_POLL_32BITS");
    }
    if features & IORING_FEAT_SQPOLL_NONFIXED != 0 {
        feats.push("IORING_FEAT_SQPOLL_NONFIXED");
    }
    if features & IORING_FEAT_EXT_ARG != 0 {
        feats.push("IORING_FEAT_EXT_ARG");
    }

    println!("[io_uring] Detected features: {}", feats.join(" | "));
}

/*
 * Library interface to io_uring
 */

// struct io_uring_cq {
// 	unsigned *khead;
// 	unsigned *ktail;
// 	unsigned *kring_mask;
// 	unsigned *kring_entries;
// 	unsigned *kflags;
// 	unsigned *koverflow;
// 	struct io_uring_cqe *cqes;

// 	size_t ring_sz;
// 	void *ring_ptr;

// 	unsigned pad[4];
// };
// struct io_uring_sq {
// 	unsigned *khead;
// 	unsigned *ktail;
// 	unsigned *kring_mask;
// 	unsigned *kring_entries;
// 	unsigned *kflags;
// 	unsigned *kdropped;
// 	unsigned *array;
// 	struct io_uring_sqe *sqes;

// 	unsigned sqe_head;
// 	unsigned sqe_tail;

// 	size_t ring_sz;
// 	void *ring_ptr;

// 	unsigned pad[4];
// };

struct SubmissionQueue {
    head: *mut AtomicU32,
    tail: *mut AtomicU32,
    ring_mask: *const u32,
    ring_size: usize,
}

#[derive(Debug)]
pub struct IoUring {
    fd: i32,
}

impl IoUring {
    #[allow(clippy::clippy::new_ret_no_self)]
    pub fn new() {
        let mut params = unsafe { std::mem::zeroed() };
        let io_ring = io_uring_setup(256, &mut params).unwrap();

        println!("PARAMS {:#?}", params);
        display_io_uring_features(params.features);

        let sq = mmap(
            &io_ring,
            params.sq_off.array + params.sq_entries * std::mem::size_of::<u32>() as u32,
        )
        .unwrap();

        println!("QUEUE {:p}", sq);
    }

    fn mmap_queues(&self) {
        // ptr = mmap(0, sq_off.array + sq_entries * sizeof(__u32),
        //            PROT_READ|PROT_WRITE, MAP_SHARED|MAP_POPULATE,
        //            ring_fd, IORING_OFF_SQ_RING);
    }
}
