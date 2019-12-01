
use core::arch::x86_64::{
    __m128i,
    _mm_add_epi32,
    _mm_extract_epi32,
    _mm_load_si128,
    _mm_loadu_si128,
    _mm_set_epi64x,
    _mm_set_epi32,
    _mm_sha1msg1_epu32,
    _mm_sha1msg2_epu32,
    _mm_sha1nexte_epu32,
    _mm_sha1rnds4_epu32,
    _mm_shuffle_epi32,
    _mm_shuffle_epi8,
    _mm_store_si128,
    _mm_storeu_si128,
    _mm_xor_si128,
};

//#[allow(clippy::cast_ptr_alignment, non_snake_case)]
#[allow(non_snake_case)]
#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "sha,sse2,ssse3,sse4.1")]
unsafe fn process_blocks(state: &mut State, data: &[u8]) {
    let (
        mut ABCD, mut ABCD_SAVE, mut E0, mut E0_SAVE,
        mut E1, mut MSG0, mut MSG1, mut MSG2, mut MSG3
    );

    let MASK = _mm_set_epi64x(0x0001_0203_0405_0607, 0x0809_0a0b_0c0d_0e0f);

    // Load initial values
    ABCD = _mm_load_si128(state.as_ptr());
    E0 = _mm_set_epi32(state.s4 as i32, 0, 0, 0);
    ABCD = _mm_shuffle_epi32(ABCD, 0x1B);

    for data in data.chunks_exact(64) {

        ABCD_SAVE = ABCD;
        E0_SAVE = E0;

        // Rounds 0-3
        MSG0 = _mm_loadu_si128(data.as_ptr() as *const __m128i);
        MSG0 = _mm_shuffle_epi8(MSG0, MASK);
        E0 = _mm_add_epi32(E0, MSG0);
        E1 = ABCD;
        ABCD = _mm_sha1rnds4_epu32(ABCD, E0, 0);

        // Rounds 4-7
        MSG1 = _mm_loadu_si128(data.as_ptr().offset(16) as *const __m128i);
        MSG1 = _mm_shuffle_epi8(MSG1, MASK);
        E1 = _mm_sha1nexte_epu32(E1, MSG1);
        E0 = ABCD;
        ABCD = _mm_sha1rnds4_epu32(ABCD, E1, 0);
        MSG0 = _mm_sha1msg1_epu32(MSG0, MSG1);

        // Rounds 8-11
        MSG2 = _mm_loadu_si128(data.as_ptr().offset(32) as *const __m128i);
        MSG2 = _mm_shuffle_epi8(MSG2, MASK);
        E0 = _mm_sha1nexte_epu32(E0, MSG2);
        E1 = ABCD;
        ABCD = _mm_sha1rnds4_epu32(ABCD, E0, 0);
        MSG1 = _mm_sha1msg1_epu32(MSG1, MSG2);
        MSG0 = _mm_xor_si128(MSG0, MSG2);

        // Rounds 12-15
        MSG3 = _mm_loadu_si128(data.as_ptr().offset(48) as *const __m128i);
        MSG3 = _mm_shuffle_epi8(MSG3, MASK);
        E1 = _mm_sha1nexte_epu32(E1, MSG3);
        E0 = ABCD;
        MSG0 = _mm_sha1msg2_epu32(MSG0, MSG3);
        ABCD = _mm_sha1rnds4_epu32(ABCD, E1, 0);
        MSG2 = _mm_sha1msg1_epu32(MSG2, MSG3);
        MSG1 = _mm_xor_si128(MSG1, MSG3);

        // Rounds 16-19
        E0 = _mm_sha1nexte_epu32(E0, MSG0);
        E1 = ABCD;
        MSG1 = _mm_sha1msg2_epu32(MSG1, MSG0);
        ABCD = _mm_sha1rnds4_epu32(ABCD, E0, 0);
        MSG3 = _mm_sha1msg1_epu32(MSG3, MSG0);
        MSG2 = _mm_xor_si128(MSG2, MSG0);

        // Rounds 20-23
        E1 = _mm_sha1nexte_epu32(E1, MSG1);
        E0 = ABCD;
        MSG2 = _mm_sha1msg2_epu32(MSG2, MSG1);
        ABCD = _mm_sha1rnds4_epu32(ABCD, E1, 1);
        MSG0 = _mm_sha1msg1_epu32(MSG0, MSG1);
        MSG3 = _mm_xor_si128(MSG3, MSG1);

        // Rounds 24-27
        E0 = _mm_sha1nexte_epu32(E0, MSG2);
        E1 = ABCD;
        MSG3 = _mm_sha1msg2_epu32(MSG3, MSG2);
        ABCD = _mm_sha1rnds4_epu32(ABCD, E0, 1);
        MSG1 = _mm_sha1msg1_epu32(MSG1, MSG2);
        MSG0 = _mm_xor_si128(MSG0, MSG2);

        // Rounds 28-31
        E1 = _mm_sha1nexte_epu32(E1, MSG3);
        E0 = ABCD;
        MSG0 = _mm_sha1msg2_epu32(MSG0, MSG3);
        ABCD = _mm_sha1rnds4_epu32(ABCD, E1, 1);
        MSG2 = _mm_sha1msg1_epu32(MSG2, MSG3);
        MSG1 = _mm_xor_si128(MSG1, MSG3);

        // Rounds 32-35
        E0 = _mm_sha1nexte_epu32(E0, MSG0);
        E1 = ABCD;
        MSG1 = _mm_sha1msg2_epu32(MSG1, MSG0);
        ABCD = _mm_sha1rnds4_epu32(ABCD, E0, 1);
        MSG3 = _mm_sha1msg1_epu32(MSG3, MSG0);
        MSG2 = _mm_xor_si128(MSG2, MSG0);

        // Rounds 36-39
        E1 = _mm_sha1nexte_epu32(E1, MSG1);
        E0 = ABCD;
        MSG2 = _mm_sha1msg2_epu32(MSG2, MSG1);
        ABCD = _mm_sha1rnds4_epu32(ABCD, E1, 1);
        MSG0 = _mm_sha1msg1_epu32(MSG0, MSG1);
        MSG3 = _mm_xor_si128(MSG3, MSG1);

        // Rounds 40-43
        E0 = _mm_sha1nexte_epu32(E0, MSG2);
        E1 = ABCD;
        MSG3 = _mm_sha1msg2_epu32(MSG3, MSG2);
        ABCD = _mm_sha1rnds4_epu32(ABCD, E0, 2);
        MSG1 = _mm_sha1msg1_epu32(MSG1, MSG2);
        MSG0 = _mm_xor_si128(MSG0, MSG2);

        // Rounds 44-47
        E1 = _mm_sha1nexte_epu32(E1, MSG3);
        E0 = ABCD;
        MSG0 = _mm_sha1msg2_epu32(MSG0, MSG3);
        ABCD = _mm_sha1rnds4_epu32(ABCD, E1, 2);
        MSG2 = _mm_sha1msg1_epu32(MSG2, MSG3);
        MSG1 = _mm_xor_si128(MSG1, MSG3);

        // Rounds 48-51
        E0 = _mm_sha1nexte_epu32(E0, MSG0);
        E1 = ABCD;
        MSG1 = _mm_sha1msg2_epu32(MSG1, MSG0);
        ABCD = _mm_sha1rnds4_epu32(ABCD, E0, 2);
        MSG3 = _mm_sha1msg1_epu32(MSG3, MSG0);
        MSG2 = _mm_xor_si128(MSG2, MSG0);

        // Rounds 52-55
        E1 = _mm_sha1nexte_epu32(E1, MSG1);
        E0 = ABCD;
        MSG2 = _mm_sha1msg2_epu32(MSG2, MSG1);
        ABCD = _mm_sha1rnds4_epu32(ABCD, E1, 2);
        MSG0 = _mm_sha1msg1_epu32(MSG0, MSG1);
        MSG3 = _mm_xor_si128(MSG3, MSG1);

        // Rounds 56-59
        E0 = _mm_sha1nexte_epu32(E0, MSG2);
        E1 = ABCD;
        MSG3 = _mm_sha1msg2_epu32(MSG3, MSG2);
        ABCD = _mm_sha1rnds4_epu32(ABCD, E0, 2);
        MSG1 = _mm_sha1msg1_epu32(MSG1, MSG2);
        MSG0 = _mm_xor_si128(MSG0, MSG2);

        // Rounds 60-63
        E1 = _mm_sha1nexte_epu32(E1, MSG3);
        E0 = ABCD;
        MSG0 = _mm_sha1msg2_epu32(MSG0, MSG3);
        ABCD = _mm_sha1rnds4_epu32(ABCD, E1, 3);
        MSG2 = _mm_sha1msg1_epu32(MSG2, MSG3);
        MSG1 = _mm_xor_si128(MSG1, MSG3);

        // Rounds 64-67
        E0 = _mm_sha1nexte_epu32(E0, MSG0);
        E1 = ABCD;
        MSG1 = _mm_sha1msg2_epu32(MSG1, MSG0);
        ABCD = _mm_sha1rnds4_epu32(ABCD, E0, 3);
        MSG3 = _mm_sha1msg1_epu32(MSG3, MSG0);
        MSG2 = _mm_xor_si128(MSG2, MSG0);

        // Rounds 68-71
        E1 = _mm_sha1nexte_epu32(E1, MSG1);
        E0 = ABCD;
        MSG2 = _mm_sha1msg2_epu32(MSG2, MSG1);
        ABCD = _mm_sha1rnds4_epu32(ABCD, E1, 3);
        MSG3 = _mm_xor_si128(MSG3, MSG1);

        // Rounds 72-75
        E0 = _mm_sha1nexte_epu32(E0, MSG2);
        E1 = ABCD;
        MSG3 = _mm_sha1msg2_epu32(MSG3, MSG2);
        ABCD = _mm_sha1rnds4_epu32(ABCD, E0, 3);

        // Rounds 76-79
        E1 = _mm_sha1nexte_epu32(E1, MSG3);
        E0 = ABCD;
        ABCD = _mm_sha1rnds4_epu32(ABCD, E1, 3);

        // Combine state
        E0 = _mm_sha1nexte_epu32(E0, E0_SAVE);
        ABCD = _mm_add_epi32(ABCD, ABCD_SAVE);
    }

    // Save state
    ABCD = _mm_shuffle_epi32(ABCD, 0x1B);
    _mm_store_si128(state.as_ptr() as *mut __m128i, ABCD);
    state.s4 = _mm_extract_epi32(E0, 3) as u32;
}

#[repr(C, align(16))]
struct State {
    s0: u32,
    s1: u32,
    s2: u32,
    s3: u32,
    s4: u32,
}

impl Default for State {
    fn default() -> State {
        State {
            s0: 0x6745_2301,
            s1: 0xEFCD_AB89,
            s2: 0x98BA_DCFE,
            s3: 0x1032_5476,
            s4: 0xC3D2_E1F0
        }
    }
}

impl State {
    fn as_mut_ptr(&mut self) -> *mut __m128i {
        self as *mut State as *mut __m128i
    }

    fn as_ptr(&self) -> *const __m128i {
        self as *const State as *const __m128i
    }
}

fn compute_sha1(data: &[u8]) -> [u8; 20] {

    // let mut state = [
    //     0x6745_2301,
    //     0xEFCD_AB89,
    //     0x98BA_DCFE,
    //     0x1032_5476,
    //     0xC3D2_E1F0
    // ];

    let mut state = State::default();

    // println!("ALIGN: {:?}", std::mem::align_of_val(&state2));
    // println!("ALIGN DATA: {:?}", std::mem::align_of_val(data));
    // println!("DATA: {:?} ALIGN {:?}", data.as_ptr() as usize, data.as_ptr() as usize % 16);

    let nblock = data.len() / 64;

    if nblock > 0 {
        unsafe { process_blocks(&mut state, data) };
    }

    let remainder = &data[nblock * 64..];
    let remainder_len = remainder.len();

    let nbits = data.len() * 8;
    let extra = [
        (nbits >> 56) as u8,
        (nbits >> 48) as u8,
        (nbits >> 40) as u8,
        (nbits >> 32) as u8,
        (nbits >> 24) as u8,
        (nbits >> 16) as u8,
        (nbits >> 8) as u8,
        (nbits >> 0) as u8
    ];

    let mut last = [0; 128];
    last[..remainder_len].copy_from_slice(remainder);
    last[remainder_len] = 0x80;

    if remainder_len < 56 {
        last[56..64].copy_from_slice(&extra);
        unsafe { process_blocks(&mut state, &last[..64]) };
    } else {
        last[120..128].copy_from_slice(&extra);
        unsafe { process_blocks(&mut state, &last[..]) };
    }

    [
        (state.s0 >> 24) as u8,
        (state.s0 >> 16) as u8,
        (state.s0 >>  8) as u8,
        (state.s0 >>  0) as u8,
        (state.s1 >> 24) as u8,
        (state.s1 >> 16) as u8,
        (state.s1 >>  8) as u8,
        (state.s1 >>  0) as u8,
        (state.s2 >> 24) as u8,
        (state.s2 >> 16) as u8,
        (state.s2 >>  8) as u8,
        (state.s2 >>  0) as u8,
        (state.s3 >> 24) as u8,
        (state.s3 >> 16) as u8,
        (state.s3 >>  8) as u8,
        (state.s3 >>  0) as u8,
        (state.s4 >> 24) as u8,
        (state.s4 >> 16) as u8,
        (state.s4 >>  8) as u8,
        (state.s4 >>  0) as u8,
    ]
}

pub fn sha1(data: &[u8]) -> [u8; 20] {
    #[cfg(any(target_arch = "x86_64"))]
    {
        if is_x86_feature_detected!("sha")
            && is_x86_feature_detected!("sse2")
            && is_x86_feature_detected!("ssse3")
            && is_x86_feature_detected!("sse4.1")
        {
            return compute_sha1(data);
        }
    }

    panic!("AAAA");
}

#[cfg(test)]
mod tests {
    use super::sha1;

    #[test]
    fn extern_vs_us_16k() {
        use rand::Rng;
        use rand::RngCore;

        let mut rng = rand::thread_rng();

        let mut vec = vec![0; 16 * 1024];

        rng.fill_bytes(&mut vec);

        let res1 = sha1(&vec);

        let mut m = sha1::Sha1::new();
        m.update(&vec);
        let res2 = m.digest().bytes();

        assert_eq!(res1, res2);
    }
}
