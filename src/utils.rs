
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
