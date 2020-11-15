use core::arch::x86_64::{
    __m128i, _mm_add_epi32, _mm_extract_epi32, _mm_load_si128, _mm_loadu_si128, _mm_set_epi32,
    _mm_set_epi64x, _mm_sha1msg1_epu32, _mm_sha1msg2_epu32, _mm_sha1nexte_epu32,
    _mm_sha1rnds4_epu32, _mm_shuffle_epi32, _mm_shuffle_epi8, _mm_store_si128, _mm_xor_si128,
};

#[allow(clippy::cast_ptr_alignment, non_snake_case)]
#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "sha,sse2,ssse3,sse4.1")]
unsafe fn process_blocks<'b>(
    state: &mut State,
    blocks: impl Iterator<Item = &'b [u8]>,
    load: unsafe fn(*const __m128i) -> __m128i,
) {
    let (
        mut ABCD,
        mut ABCD_SAVE,
        mut E0,
        mut E0_SAVE,
        mut E1,
        mut MSG0,
        mut MSG1,
        mut MSG2,
        mut MSG3,
    );

    let MASK = _mm_set_epi64x(0x0001_0203_0405_0607, 0x0809_0a0b_0c0d_0e0f);

    // Load initial values
    ABCD = _mm_load_si128(state.as_ptr());
    E0 = _mm_set_epi32(state.s4 as i32, 0, 0, 0);
    ABCD = _mm_shuffle_epi32(ABCD, 0x1B);

    for block in blocks {
        ABCD_SAVE = ABCD;
        E0_SAVE = E0;

        // Rounds 0-3
        MSG0 = load(block.as_ptr() as *const __m128i);
        MSG0 = _mm_shuffle_epi8(MSG0, MASK);
        E0 = _mm_add_epi32(E0, MSG0);
        E1 = ABCD;
        ABCD = _mm_sha1rnds4_epu32(ABCD, E0, 0);

        // Rounds 4-7
        MSG1 = load(block.as_ptr().offset(16) as *const __m128i);
        MSG1 = _mm_shuffle_epi8(MSG1, MASK);
        E1 = _mm_sha1nexte_epu32(E1, MSG1);
        E0 = ABCD;
        ABCD = _mm_sha1rnds4_epu32(ABCD, E1, 0);
        MSG0 = _mm_sha1msg1_epu32(MSG0, MSG1);

        // Rounds 8-11
        MSG2 = load(block.as_ptr().offset(32) as *const __m128i);
        MSG2 = _mm_shuffle_epi8(MSG2, MASK);
        E0 = _mm_sha1nexte_epu32(E0, MSG2);
        E1 = ABCD;
        ABCD = _mm_sha1rnds4_epu32(ABCD, E0, 0);
        MSG1 = _mm_sha1msg1_epu32(MSG1, MSG2);
        MSG0 = _mm_xor_si128(MSG0, MSG2);

        // Rounds 12-15
        MSG3 = load(block.as_ptr().offset(48) as *const __m128i);
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
    _mm_store_si128(state.as_mut_ptr(), ABCD);
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
            s4: 0xC3D2_E1F0,
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

#[repr(C, align(16))]
struct Padding {
    padding: [u8; 128],
    end: usize,
}

impl Padding {
    fn new(rest: &[u8], nbits: usize) -> Padding {
        let mut padding = [0; 128];

        let nbits_be = nbits.to_be_bytes();
        let rest_len = rest.len();

        padding[..rest_len].copy_from_slice(rest);
        padding[rest_len] = 0x80;

        let end = if rest_len < 56 {
            padding[56..64].copy_from_slice(&nbits_be);
            64
        } else {
            padding[120..].copy_from_slice(&nbits_be);
            128
        };

        Padding { padding, end }
    }
}

use std::ops::Deref;

impl Deref for Padding {
    type Target = [u8];

    fn deref(&self) -> &[u8] {
        &self.padding[..self.end]
    }
}

#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "sha,sse2,ssse3,sse4.1")]
pub(super) unsafe fn compute_sha1(data: &[u8]) -> [u8; 20] {
    // Initial state
    let mut state = State::default();

    // number of bits in the data
    let nbits = data.len() * 8;

    // data by chunks of 64 bytes
    let data_blocks = data.chunks_exact(64);

    // Rest of the data
    let rest = data_blocks.remainder();

    // Padding
    let pad = Padding::new(rest, nbits);

    // Iterator on data + padding, by chunks of 64 bytes
    let blocks = data_blocks.chain(pad.chunks_exact(64));

    // The compiler optimize this and make process_block inlined
    if data.as_ptr() as usize % 16 == 0 {
        // data is aligned, use movdqa
        process_blocks(&mut state, blocks, _mm_load_si128);
    } else {
        // data is unaligned, use movdqu
        process_blocks(&mut state, blocks, _mm_loadu_si128);
    }

    #[allow(clippy::identity_op)]
    [
        (state.s0 >> 24) as u8,
        (state.s0 >> 16) as u8,
        (state.s0 >> 8) as u8,
        (state.s0 >> 0) as u8,
        (state.s1 >> 24) as u8,
        (state.s1 >> 16) as u8,
        (state.s1 >> 8) as u8,
        (state.s1 >> 0) as u8,
        (state.s2 >> 24) as u8,
        (state.s2 >> 16) as u8,
        (state.s2 >> 8) as u8,
        (state.s2 >> 0) as u8,
        (state.s3 >> 24) as u8,
        (state.s3 >> 16) as u8,
        (state.s3 >> 8) as u8,
        (state.s3 >> 0) as u8,
        (state.s4 >> 24) as u8,
        (state.s4 >> 16) as u8,
        (state.s4 >> 8) as u8,
        (state.s4 >> 0) as u8,
    ]
}
