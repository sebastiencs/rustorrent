#![feature(test)]
//#[cfg(feature = "benchmark-internals")]

extern crate test;

use test::Bencher;

#[bench]
fn slice_compare_equal(b: &mut test::Bencher) {
    let vec1 = vec![5; 20];
    let vec2 = vec1.clone();

    b.iter(|| {
        assert!(&vec2 == &vec1);
    });
    b.bytes = vec1.len() as u64;
}

use rustorrent::actors::sha1::compare_sum;

#[bench]
fn simd_compare_equal(b: &mut Bencher) {
    let vec1 = vec![5; 20];
    let vec2 = vec1.clone();

    b.iter(|| {
        assert!(compare_sum(&vec1, &vec2))
    });
    b.bytes = vec1.len() as u64;
}

#[bench]
fn sha1_string_extern_crate(b: &mut Bencher) {
    use sha1::Sha1;

    let mut msg = b"The quick brown fox jumps over the lazy dog";

    b.iter(|| {
        let mut m = Sha1::new();
        m.update(&msg[..]);
        m.digest()
    });
    b.bytes = msg.len() as u64;
}

#[bench]
fn sha1_16K_extern_crate(b: &mut Bencher) {
    use sha1::Sha1;

    use rand::Rng;
    use rand::RngCore;

    let mut rng = rand::thread_rng();
    let mut vec = vec![0; 16 * 1024];

    rng.fill_bytes(&mut vec);

    b.iter(|| {
        let mut m = Sha1::new();
        m.update(&vec);
        m.digest()
    });
    b.bytes = vec.len() as u64;
}

#[bench]
fn sha1_string(b: &mut Bencher) {
    use rustorrent::sha1::sha1;

    let mut msg = b"The quick brown fox jumps over the lazy dog";

    b.iter(|| {
        sha1(&msg[..])
    });
    b.bytes = msg.len() as u64;
}

#[bench]
fn sha1_16K(b: &mut Bencher) {
    use rustorrent::sha1::sha1;

    use rand::Rng;
    use rand::RngCore;

    let mut rng = rand::thread_rng();
    let mut vec = vec![0; 16 * 1024];

    rng.fill_bytes(&mut vec);

    b.iter(|| {
        sha1(&vec)
    });
    b.bytes = vec.len() as u64;
}

#[bench]
fn sha1_512K(b: &mut Bencher) {
    use rustorrent::sha1::sha1;

    use rand::Rng;
    use rand::RngCore;

    let mut rng = rand::thread_rng();
    let mut vec = vec![0; 512 * 1024];

    rng.fill_bytes(&mut vec);

    b.iter(|| {
        sha1(&vec)
    });
    b.bytes = vec.len() as u64;
}

#[bench]
fn sha1_512K_unaligned(b: &mut Bencher) {
    use rustorrent::sha1::sha1;

    use rand::Rng;
    use rand::RngCore;

    let mut rng = rand::thread_rng();
    let mut vec = vec![0; 512 * 1024];

    rng.fill_bytes(&mut vec);

    b.iter(|| {
        sha1(&vec[6..])
    });
    b.bytes = vec.len() as u64;
}

// TODO: Sha1 with Simd
// https://github.com/noloader/SHA-Intrinsics
// https://www.nayuki.io/page/fast-sha1-hash-implementation-in-x86-assembly
// https://software.intel.com/en-us/articles/intel-sha-extensions
// https://doc.rust-lang.org/stable/edition-guide/rust-2018/simd-for-faster-computing.html
// https://doc.rust-lang.org/nightly/core/arch/index.html
