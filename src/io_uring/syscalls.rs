use std::{os::unix::io::RawFd, ptr::NonNull};

use libc::{c_void, off_t, syscall};

use super::types::{
    io_uring_params, EnterFlags, IoUringFd, SYS_IO_URING_ENTER, SYS_IO_URING_REGISTER,
    SYS_IO_URING_SETUP,
};

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
    flags: EnterFlags,
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
                flags.bits(),
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
