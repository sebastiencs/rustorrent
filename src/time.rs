
// TODO: Looks like IOS 10 has clock_gettime():
// https://chromium.googlesource.com/chromium/src/base/+/master/time/time_mac.cc
// macOS Sierra 10.12 too:
// https://stackoverflow.com/a/39801564
// https://opensource.apple.com/source/Libc/Libc-1158.1.2/gen/clock_gettime.c.auto.html

#[cfg(any(target_os = "macos", target_os = "ios"))]
#[allow(non_camel_case_types)]
pub fn get_time() -> (u64, u64) {
    use std::sync::atomic::{AtomicUsize, Ordering::SeqCst};

    extern "C" {
        fn mach_absolute_time() -> u64;
    }

    #[repr(C)]
    #[derive(Copy, Clone)]
    struct mach_timebase_info {
        numer: u32,
        denom: u32,
    }
    type mach_timebase_info_t = *mut mach_timebase_info;
    type kern_return_t = libc::c_int;

    static mut START: u64 = 0;

    fn get_absolute_time() -> u64 {
        unsafe { mach_absolute_time() }
    }

    // This is extracted from rust libstd
    fn info() -> mach_timebase_info {
        static mut INFO: mach_timebase_info = mach_timebase_info {
            numer: 0,
            denom: 0,
        };
        static STATE: AtomicUsize = AtomicUsize::new(0);

        unsafe {
            // If a previous thread has filled in this global state, use that.
            if STATE.load(SeqCst) == 2 {
                return INFO;
            }

            // ... otherwise learn for ourselves ...
            let mut info = std::mem::zeroed();
            extern "C" {
                fn mach_timebase_info(info: mach_timebase_info_t) -> kern_return_t;
            }

            mach_timebase_info(&mut info);

            // ... and attempt to be the one thread that stores it globally for
            // all other threads
            if STATE.compare_exchange(0, 1, SeqCst, SeqCst).is_ok() {
                INFO = info;
                START = get_absolute_time();
                STATE.store(2, SeqCst);
            }
            info
        }
    }

    let info = info();

    let time: u64 = get_absolute_time();

    unsafe {
        (0 ,((time - START) * info.numer as u64) / info.denom as u64)
    }
}

#[cfg(all(unix, not(any(target_os = "macos", target_os = "ios"))))]
pub fn get_time() -> (u64, u64) {
    let mut t = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };

    let val = unsafe {
        libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut t)
    };

    // This might return -1 with old kernels
    if val >= 0 {
        return (t.tv_sec as u64, t.tv_nsec as u64);
    }

    let mut s = libc::timeval {
        tv_sec: 0,
        tv_usec: 0,
    };

    unsafe {
        libc::gettimeofday(&mut s, std::ptr::null_mut());
    }

    (s.tv_sec as u64, (s.tv_usec * 1000) as u64)
}

#[cfg(windows)]
pub fn get_times() -> (u64, u64) {
    extern "system" {
        pub fn GetTickCount64() -> libc::c_ulonglong;
    }

    (0, unsafe { GetTickCount64() } * 1000)
}
