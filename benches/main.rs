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
