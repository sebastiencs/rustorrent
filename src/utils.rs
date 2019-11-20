
pub trait FromSlice<T> {
    fn from_slice(slice: &[T]) -> Vec<T>;
}

impl<T: Copy> FromSlice<T> for Vec<T> {
    fn from_slice(slice: &[T]) -> Vec<T> {
        let len = slice.len();
        let mut vec = Vec::with_capacity(len);
        unsafe { vec.set_len(len); }
        vec.as_mut_slice().copy_from_slice(slice);
        vec
    }
}

#[derive(Default)]
pub struct NoHash(usize);

impl std::hash::BuildHasher for NoHash {
    type Hasher = Self;
    fn build_hasher(&self) -> Self::Hasher {
        Self(0)
    }
}

impl std::hash::Hasher for NoHash {
    fn finish(&self) -> u64 {
        self.0 as u64
    }
    fn write(&mut self, _: &[u8]) {
        unreachable!()
    }
    fn write_usize(&mut self, n: usize) {
        self.0 = n
    }
}
