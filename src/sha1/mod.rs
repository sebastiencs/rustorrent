
mod x86_sha;

pub fn sha1(data: &[u8]) -> [u8; 20] {
    #[cfg(any(target_arch = "x86_64"))]
    {
        if is_x86_feature_detected!("sha")
            && is_x86_feature_detected!("sse2")
            && is_x86_feature_detected!("ssse3")
            && is_x86_feature_detected!("sse4.1")
        {
            return unsafe { x86_sha::compute_sha1(data) }
        }
    }

    panic!("AAAA");
}
