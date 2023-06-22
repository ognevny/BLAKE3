use crate::platform::{ParentInOut, TransposedVectors, MAX_SIMD_DEGREE};
use crate::{
    CVBytes, CVWords, IncrementCounter, BLOCK_LEN, CHUNK_LEN, OUT_LEN, UNIVERSAL_HASH_LEN,
};

use arrayref::array_ref;
use arrayvec::ArrayVec;
use core::cmp;
use core::usize;
use rand::prelude::*;

// Interesting input lengths to run tests on.
pub const TEST_CASES: &[usize] = &[
    0,
    1,
    2,
    3,
    4,
    5,
    6,
    7,
    8,
    BLOCK_LEN - 1,
    BLOCK_LEN,
    BLOCK_LEN + 1,
    2 * BLOCK_LEN - 1,
    2 * BLOCK_LEN,
    2 * BLOCK_LEN + 1,
    CHUNK_LEN - 1,
    CHUNK_LEN,
    CHUNK_LEN + 1,
    2 * CHUNK_LEN,
    2 * CHUNK_LEN + 1,
    3 * CHUNK_LEN,
    3 * CHUNK_LEN + 1,
    4 * CHUNK_LEN,
    4 * CHUNK_LEN + 1,
    5 * CHUNK_LEN,
    5 * CHUNK_LEN + 1,
    6 * CHUNK_LEN,
    6 * CHUNK_LEN + 1,
    7 * CHUNK_LEN,
    7 * CHUNK_LEN + 1,
    8 * CHUNK_LEN,
    8 * CHUNK_LEN + 1,
    16 * CHUNK_LEN,  // AVX512's bandwidth
    31 * CHUNK_LEN,  // 16 + 8 + 4 + 2 + 1
    100 * CHUNK_LEN, // subtrees larger than MAX_SIMD_DEGREE chunks
];

pub const TEST_CASES_MAX: usize = 100 * CHUNK_LEN;

// There's a test to make sure these two are equal below.
pub const TEST_KEY: &CVBytes = b"whats the Elvish word for friend";
pub const TEST_KEY_WORDS: &CVWords = &[
    1952540791, 1752440947, 1816469605, 1752394102, 1919907616, 1868963940, 1919295602, 1684956521,
];

// Test a few different initial counter values.
// - 0: The base case.
// - i32::MAX: *No* overflow. But carry bugs in tricky SIMD code can screw this up, if you XOR
//   when you're supposed to ANDNOT...
// - u32::MAX: The low word of the counter overflows for all inputs except the first.
const INITIAL_COUNTERS: &[u64] = &[0, i32::MAX as u64, u32::MAX as u64];

// Paint the input with a repeating byte pattern. We use a cycle length of 251,
// because that's the largest prime number less than 256. This makes it
// unlikely to swapping any two adjacent input blocks or chunks will give the
// same answer.
pub fn paint_test_input(buf: &mut [u8]) {
    for (i, b) in buf.iter_mut().enumerate() {
        *b = (i % 251) as u8;
    }
}

type CompressInPlaceFn =
    unsafe fn(cv: &mut CVWords, block: &[u8; BLOCK_LEN], block_len: u8, counter: u64, flags: u8);

type CompressXofFn = unsafe fn(
    cv: &CVWords,
    block: &[u8; BLOCK_LEN],
    block_len: u8,
    counter: u64,
    flags: u8,
) -> [u8; 64];

// A shared helper function for platform-specific tests.
pub fn test_compress_fn(compress_in_place_fn: CompressInPlaceFn, compress_xof_fn: CompressXofFn) {
    let initial_state = *TEST_KEY_WORDS;
    let block_len: u8 = 61;
    let mut block = [0; BLOCK_LEN];
    paint_test_input(&mut block[..block_len as usize]);
    // Use a counter with set bits in both 32-bit words.
    let counter = (5u64 << 32) + 6;
    let flags = crate::CHUNK_END | crate::ROOT | crate::KEYED_HASH;

    let portable_out =
        crate::portable::compress_xof(&initial_state, &block, block_len, counter as u64, flags);

    let mut test_state = initial_state;
    unsafe { compress_in_place_fn(&mut test_state, &block, block_len, counter as u64, flags) };
    let test_state_bytes = crate::platform::le_bytes_from_words_32(&test_state);
    let test_xof =
        unsafe { compress_xof_fn(&initial_state, &block, block_len, counter as u64, flags) };

    assert_eq!(&portable_out[..32], &test_state_bytes[..]);
    assert_eq!(&portable_out[..], &test_xof[..]);
}

type HashManyFn<A> = unsafe fn(
    inputs: &[&A],
    key: &CVWords,
    counter: u64,
    increment_counter: IncrementCounter,
    flags: u8,
    flags_start: u8,
    flags_end: u8,
    out: &mut [u8],
);

// A shared helper function for platform-specific tests.
pub fn test_hash_many_fn(
    hash_many_chunks_fn: HashManyFn<[u8; CHUNK_LEN]>,
    hash_many_parents_fn: HashManyFn<[u8; 2 * OUT_LEN]>,
) {
    for &counter in INITIAL_COUNTERS {
        #[cfg(feature = "std")]
        dbg!(counter);

        // 31 (16 + 8 + 4 + 2 + 1) inputs
        const NUM_INPUTS: usize = 31;
        let mut input_buf = [0; CHUNK_LEN * NUM_INPUTS];
        paint_test_input(&mut input_buf);

        // First hash chunks.
        let mut chunks = ArrayVec::<&[u8; CHUNK_LEN], NUM_INPUTS>::new();
        for i in 0..NUM_INPUTS {
            chunks.push(array_ref!(input_buf, i * CHUNK_LEN, CHUNK_LEN));
        }
        let mut portable_chunks_out = [0; NUM_INPUTS * OUT_LEN];
        crate::portable::hash_many(
            &chunks,
            TEST_KEY_WORDS,
            counter,
            IncrementCounter::Yes,
            crate::KEYED_HASH,
            crate::CHUNK_START,
            crate::CHUNK_END,
            &mut portable_chunks_out,
        );

        let mut test_chunks_out = [0; NUM_INPUTS * OUT_LEN];
        unsafe {
            hash_many_chunks_fn(
                &chunks[..],
                TEST_KEY_WORDS,
                counter,
                IncrementCounter::Yes,
                crate::KEYED_HASH,
                crate::CHUNK_START,
                crate::CHUNK_END,
                &mut test_chunks_out,
            );
        }
        for n in 0..NUM_INPUTS {
            #[cfg(feature = "std")]
            dbg!(n);
            assert_eq!(
                &portable_chunks_out[n * OUT_LEN..][..OUT_LEN],
                &test_chunks_out[n * OUT_LEN..][..OUT_LEN]
            );
        }

        // Then hash parents.
        let mut parents = ArrayVec::<&[u8; 2 * OUT_LEN], NUM_INPUTS>::new();
        for i in 0..NUM_INPUTS {
            parents.push(array_ref!(input_buf, i * 2 * OUT_LEN, 2 * OUT_LEN));
        }
        let mut portable_parents_out = [0; NUM_INPUTS * OUT_LEN];
        crate::portable::hash_many(
            &parents,
            TEST_KEY_WORDS,
            counter,
            IncrementCounter::No,
            crate::KEYED_HASH | crate::PARENT,
            0,
            0,
            &mut portable_parents_out,
        );

        let mut test_parents_out = [0; NUM_INPUTS * OUT_LEN];
        unsafe {
            hash_many_parents_fn(
                &parents[..],
                TEST_KEY_WORDS,
                counter,
                IncrementCounter::No,
                crate::KEYED_HASH | crate::PARENT,
                0,
                0,
                &mut test_parents_out,
            );
        }
        for n in 0..NUM_INPUTS {
            #[cfg(feature = "std")]
            dbg!(n);
            assert_eq!(
                &portable_parents_out[n * OUT_LEN..][..OUT_LEN],
                &test_parents_out[n * OUT_LEN..][..OUT_LEN]
            );
        }
    }
}

// Both xof() and xof_xof() have this signature.
type HashChunksFn = unsafe fn(
    input: *const u8,
    input_len: usize,
    key: *const u32,
    initial_counter: u64,
    counter_group: u64,
    flags: u32,
    transposed_output: *mut u32,
);

pub fn test_hash_chunks_fn(target_fn: HashChunksFn, degree: usize) {
    assert!(degree <= MAX_SIMD_DEGREE);
    let mut input = [0u8; 2 * MAX_SIMD_DEGREE * CHUNK_LEN];
    paint_test_input(&mut input);
    for test_degree in 1..=degree {
        let input1 = &input[..test_degree * CHUNK_LEN];
        let input2 = &input[test_degree * CHUNK_LEN..][..test_degree * CHUNK_LEN];
        for &initial_counter in INITIAL_COUNTERS {
            // Make two calls, to test the output_column parameter.
            let mut test_output = TransposedVectors::default();
            unsafe {
                target_fn(
                    input1.as_ptr(),
                    input1.len(),
                    TEST_KEY_WORDS.as_ptr(),
                    initial_counter,
                    0,
                    crate::KEYED_HASH as u32,
                    test_output[0].as_mut_ptr(),
                );
                target_fn(
                    input2.as_ptr(),
                    input2.len(),
                    TEST_KEY_WORDS.as_ptr(),
                    initial_counter + test_degree as u64,
                    0,
                    crate::KEYED_HASH as u32,
                    test_output[0].as_mut_ptr().add(test_degree),
                );
            }

            let mut portable_output = TransposedVectors::default();
            unsafe {
                crate::portable::hash_chunks(
                    input1.as_ptr(),
                    input1.len(),
                    TEST_KEY_WORDS.as_ptr(),
                    initial_counter,
                    0,
                    crate::KEYED_HASH as u32,
                    test_output[0].as_mut_ptr(),
                );
                crate::portable::hash_chunks(
                    input2.as_ptr(),
                    input2.len(),
                    TEST_KEY_WORDS.as_ptr(),
                    initial_counter + test_degree as u64,
                    0,
                    crate::KEYED_HASH as u32,
                    test_output[0].as_mut_ptr().add(test_degree),
                );
            }

            assert_eq!(portable_output, test_output);
        }
    }
}

fn paint_transposed_input(input: &mut TransposedVectors) {
    let mut val = 0;
    for row in 0..8 {
        for col in 0..2 * MAX_SIMD_DEGREE {
            input[row][col] = val;
            val += 1;
        }
    }
}

// Both xof() and xof_xof() have this signature.
type HashParentsFn = unsafe fn(
    transposed_input: *const u32,
    num_parents: usize,
    key: *const u32,
    flags: u32,
    transposed_output: *mut u32, // may overlap the input
);

pub fn test_hash_parents_fn(target_fn: HashParentsFn, degree: usize) {
    assert!(degree <= MAX_SIMD_DEGREE);
    for test_degree in 1..=degree {
        // separate
        {
            let mut input = TransposedVectors::default();
            paint_transposed_input(&mut input);
            let mut test_output = input.clone();
            unsafe {
                target_fn(
                    ParentInOut::Separate {
                        input: &input,
                        num_parents: test_degree,
                        output: &mut test_output,
                        output_column: 0,
                    },
                    TEST_KEY_WORDS,
                    crate::KEYED_HASH | crate::PARENT,
                );
            }

            let mut portable_output = TransposedVectors(input.0);
            crate::portable::hash_parents(
                ParentInOut::Separate {
                    input: &input,
                    num_parents: test_degree,
                    output: &mut portable_output,
                    output_column: 0,
                },
                TEST_KEY_WORDS,
                crate::KEYED_HASH | crate::PARENT,
            );

            assert_eq!(portable_output.0, test_output.0);
        }

        // in-place
        {
            let mut test_io = TransposedVectors::default();
            paint_transposed_input(&mut test_io);
            unsafe {
                target_fn(
                    ParentInOut::InPlace {
                        in_out: &mut test_io,
                        num_parents: test_degree,
                    },
                    TEST_KEY_WORDS,
                    crate::KEYED_HASH | crate::PARENT,
                );
            }

            let mut portable_io = TransposedVectors::default();
            paint_transposed_input(&mut portable_io);
            crate::portable::hash_parents(
                ParentInOut::InPlace {
                    in_out: &mut portable_io,
                    num_parents: test_degree,
                },
                TEST_KEY_WORDS,
                crate::KEYED_HASH | crate::PARENT,
            );

            assert_eq!(portable_io.0, test_io.0);
        }
    }
}

fn hash_with_chunks_and_parents_recurse(
    chunks_fn: HashChunksFn,
    parents_fn: HashParentsFn,
    degree: usize,
    input: &[u8],
    counter: u64,
    output: &mut TransposedVectors,
    output_column: usize,
) -> usize {
    // TODO: hash partial chunks?
    assert_eq!(input.len() % CHUNK_LEN, 0);
    assert_eq!(degree.count_ones(), 1, "power of 2");
    if input.len() <= degree * CHUNK_LEN {
        unsafe {
            chunks_fn(input, crate::IV, counter, 0, output, output_column);
        }
        input.len() / CHUNK_LEN
    } else {
        let mut child_output = TransposedVectors::default();
        let (left_input, right_input) = input.split_at(crate::left_len(input.len()));
        let mut children = hash_with_chunks_and_parents_recurse(
            chunks_fn,
            parents_fn,
            degree,
            left_input,
            counter,
            &mut child_output,
            0,
        );
        assert_eq!(children, degree);
        children += hash_with_chunks_and_parents_recurse(
            chunks_fn,
            parents_fn,
            degree,
            right_input,
            counter + (left_input.len() / CHUNK_LEN) as u64,
            &mut child_output,
            children,
        );
        unsafe {
            parents_fn(
                ParentInOut::Separate {
                    input: &child_output,
                    num_parents: children / 2,
                    output,
                    output_column,
                },
                crate::IV,
                crate::PARENT,
            );
        }
        // If there's an odd child left over, copy it to the output.
        if children % 2 == 0 {
            children / 2
        } else {
            for i in 0..8 {
                output[i][output_column + (children / 2)] = child_output[i][children - 1];
            }
            (children / 2) + 1
        }
    }
}

fn root_hash_with_chunks_and_parents(
    chunks_fn: HashChunksFn,
    parents_fn: HashParentsFn,
    degree: usize,
    input: &[u8],
) -> [u8; 32] {
    assert_eq!(degree.count_ones(), 1, "power of 2");
    // TODO: handle the 1-chunk case?
    assert!(input.len() >= 2 * CHUNK_LEN);
    // TODO: hash partial chunks?
    assert_eq!(input.len() % CHUNK_LEN, 0);
    let mut cvs = TransposedVectors::default();
    let mut num_cvs =
        hash_with_chunks_and_parents_recurse(chunks_fn, parents_fn, degree, input, 0, &mut cvs, 0);
    while num_cvs > 2 {
        unsafe {
            parents_fn(
                ParentInOut::InPlace {
                    in_out: &mut cvs,
                    num_parents: num_cvs / 2,
                },
                crate::IV,
                crate::PARENT,
            );
        }
        if num_cvs % 2 == 0 {
            num_cvs = num_cvs / 2;
        } else {
            for i in 0..8 {
                cvs[i][num_cvs / 2] = cvs[i][num_cvs - 1];
            }
            num_cvs = (num_cvs / 2) + 1;
        }
    }
    unsafe {
        parents_fn(
            ParentInOut::InPlace {
                in_out: &mut cvs,
                num_parents: 1,
            },
            crate::IV,
            crate::PARENT | crate::ROOT,
        );
    }
    let mut ret = [0u8; 32];
    for i in 0..8 {
        ret[4 * i..][..4].copy_from_slice(&cvs[i][0].to_le_bytes());
    }
    ret
}

#[test]
pub fn test_compare_reference_impl_chunks_and_hashes() {
    // 31 (16 + 8 + 4 + 2 + 1) chunks
    const MAX_CHUNKS: usize = 31;
    let mut input = [0u8; MAX_CHUNKS * CHUNK_LEN];
    paint_test_input(&mut input);
    for num_chunks in 2..=MAX_CHUNKS {
        #[cfg(feature = "std")]
        dbg!(num_chunks);

        let mut reference_output = [0u8; 32];
        let mut reference_hasher = reference_impl::Hasher::new();
        reference_hasher.update(&input[..num_chunks * CHUNK_LEN]);
        reference_hasher.finalize(&mut reference_output);

        for test_degree in [2, 4, 8, 16] {
            let test_output = root_hash_with_chunks_and_parents(
                crate::portable::hash_chunks,
                crate::portable::hash_parents,
                test_degree,
                &input[..num_chunks * CHUNK_LEN],
            );
            assert_eq!(reference_output, test_output);
        }
    }
}

// Both xof() and xof_xof() have this signature.
type XofFn = unsafe fn(
    block: &[u8; BLOCK_LEN],
    block_len: u8,
    cv: &[u32; 8],
    counter: u64,
    flags: u8,
    out: &mut [u8],
);

pub fn test_xof_and_xor_fns(target_xof: XofFn, target_xof_xor: XofFn) {
    // 31 (16 + 8 + 4 + 2 + 1) outputs
    const NUM_OUTPUTS: usize = 31;
    let different_flags = [
        crate::CHUNK_START | crate::CHUNK_END | crate::ROOT,
        crate::PARENT | crate::ROOT | crate::KEYED_HASH,
    ];
    for input_len in [0, 1, BLOCK_LEN] {
        let mut input_block = [0u8; BLOCK_LEN];
        paint_test_input(&mut input_block[..input_len]);
        for output_len in [0, 1, BLOCK_LEN, BLOCK_LEN + 1, BLOCK_LEN * NUM_OUTPUTS] {
            let mut test_output_buf = [0xff; BLOCK_LEN * NUM_OUTPUTS];
            for &counter in INITIAL_COUNTERS {
                for flags in different_flags {
                    let mut expected_output_buf = [0xff; BLOCK_LEN * NUM_OUTPUTS];
                    crate::portable::xof(
                        &input_block,
                        input_len as u8,
                        TEST_KEY_WORDS,
                        counter,
                        flags,
                        &mut expected_output_buf[..output_len],
                    );

                    unsafe {
                        target_xof(
                            &input_block,
                            input_len as u8,
                            TEST_KEY_WORDS,
                            counter,
                            flags,
                            &mut test_output_buf[..output_len],
                        );
                    }
                    assert_eq!(
                        expected_output_buf[..output_len],
                        test_output_buf[..output_len],
                    );
                    // Make sure unsafe implementations don't overwrite. This shouldn't be possible in the
                    // portable implementation, which is all safe code, but it could happen in others.
                    assert!(test_output_buf[output_len..].iter().all(|&b| b == 0xff));

                    // The first XOR cancels out the output.
                    unsafe {
                        target_xof_xor(
                            &input_block,
                            input_len as u8,
                            TEST_KEY_WORDS,
                            counter,
                            flags,
                            &mut test_output_buf[..output_len],
                        );
                    }
                    assert!(test_output_buf[..output_len].iter().all(|&b| b == 0));
                    assert!(test_output_buf[output_len..].iter().all(|&b| b == 0xff));

                    // The second XOR restores out the output.
                    unsafe {
                        target_xof_xor(
                            &input_block,
                            input_len as u8,
                            TEST_KEY_WORDS,
                            counter,
                            flags,
                            &mut test_output_buf[..output_len],
                        );
                    }
                    assert_eq!(
                        expected_output_buf[..output_len],
                        test_output_buf[..output_len],
                    );
                    assert!(test_output_buf[output_len..].iter().all(|&b| b == 0xff));
                }
            }
        }
    }
}

#[test]
fn test_compare_reference_impl_xof() {
    const NUM_OUTPUTS: usize = 31;
    let input = b"hello world";
    let mut input_block = [0; BLOCK_LEN];
    input_block[..input.len()].copy_from_slice(input);

    let mut reference_output_buf = [0; BLOCK_LEN * NUM_OUTPUTS];
    let mut reference_hasher = reference_impl::Hasher::new_keyed(TEST_KEY);
    reference_hasher.update(input);
    reference_hasher.finalize(&mut reference_output_buf);

    for output_len in [0, 1, BLOCK_LEN, BLOCK_LEN + 1, BLOCK_LEN * NUM_OUTPUTS] {
        let mut test_output_buf = [0; BLOCK_LEN * NUM_OUTPUTS];
        crate::platform::Platform::detect().xof(
            &input_block,
            input.len() as u8,
            TEST_KEY_WORDS,
            0,
            crate::KEYED_HASH | crate::CHUNK_START | crate::CHUNK_END | crate::ROOT,
            &mut test_output_buf[..output_len],
        );
        assert_eq!(
            reference_output_buf[..output_len],
            test_output_buf[..output_len],
        );

        // Make sure unsafe implementations don't overwrite. This shouldn't be possible in the
        // portable implementation, which is all safe code, but it could happen in others.
        assert!(test_output_buf[output_len..].iter().all(|&b| b == 0));

        // Do it again starting from block 1.
        if output_len >= BLOCK_LEN {
            crate::platform::Platform::detect().xof(
                &input_block,
                input.len() as u8,
                TEST_KEY_WORDS,
                1,
                crate::KEYED_HASH | crate::CHUNK_START | crate::CHUNK_END | crate::ROOT,
                &mut test_output_buf[..output_len - BLOCK_LEN],
            );
            assert_eq!(
                reference_output_buf[BLOCK_LEN..output_len],
                test_output_buf[..output_len - BLOCK_LEN],
            );
        }
    }
}

type UniversalHashFn =
    unsafe fn(input: &[u8], key: &[u32; 8], counter: u64) -> [u8; UNIVERSAL_HASH_LEN];

pub fn test_universal_hash_fn(target_fn: UniversalHashFn) {
    // 31 (16 + 8 + 4 + 2 + 1) inputs
    const NUM_INPUTS: usize = 31;
    let mut input_buf = [0; BLOCK_LEN * NUM_INPUTS];
    paint_test_input(&mut input_buf);
    for len in [0, 1, BLOCK_LEN, BLOCK_LEN + 1, input_buf.len()] {
        for &counter in INITIAL_COUNTERS {
            let portable_output =
                crate::portable::universal_hash(&input_buf[..len], TEST_KEY_WORDS, counter);
            let test_output = unsafe { target_fn(&input_buf[..len], TEST_KEY_WORDS, counter) };
            assert_eq!(portable_output, test_output);
        }
    }
}

fn reference_impl_universal_hash(
    input: &[u8],
    key: &[u8; crate::KEY_LEN],
) -> [u8; UNIVERSAL_HASH_LEN] {
    // The reference_impl doesn't support XOF seeking, so we have to materialize an entire extended
    // output to seek to a block.
    const MAX_BLOCKS: usize = 31;
    assert!(input.len() / BLOCK_LEN <= MAX_BLOCKS);
    let mut output_buffer: [u8; BLOCK_LEN * MAX_BLOCKS] = [0u8; BLOCK_LEN * MAX_BLOCKS];
    let mut result = [0u8; UNIVERSAL_HASH_LEN];
    let mut i = 0;
    while i == 0 || i < input.len() {
        let block_len = cmp::min(input.len() - i, BLOCK_LEN);
        let mut reference_hasher = reference_impl::Hasher::new_keyed(key);
        reference_hasher.update(&input[i..i + block_len]);
        reference_hasher.finalize(&mut output_buffer);
        for (result_byte, output_byte) in result
            .iter_mut()
            .zip(output_buffer[i..i + UNIVERSAL_HASH_LEN].iter())
        {
            *result_byte ^= *output_byte;
        }
        i += BLOCK_LEN;
    }
    result
}

#[test]
fn test_compare_reference_impl_universal_hash() {
    const NUM_INPUTS: usize = 31;
    let mut input_buf = [0; BLOCK_LEN * NUM_INPUTS];
    paint_test_input(&mut input_buf);
    for len in [0, 1, BLOCK_LEN, BLOCK_LEN + 1, input_buf.len()] {
        let reference_output = reference_impl_universal_hash(&input_buf[..len], TEST_KEY);
        let test_output = crate::platform::Platform::detect().universal_hash(
            &input_buf[..len],
            TEST_KEY_WORDS,
            0,
        );
        assert_eq!(reference_output, test_output);
    }
}

#[test]
fn test_key_bytes_equal_key_words() {
    assert_eq!(
        TEST_KEY_WORDS,
        &crate::platform::words_from_le_bytes_32(TEST_KEY),
    );
}

#[test]
fn test_reference_impl_size() {
    // Because the Rust compiler optimizes struct layout, it's possible that
    // some future version of the compiler will produce a different size. If
    // that happens, we can either disable this test, or test for multiple
    // expected values. For now, the purpose of this test is to make sure we
    // notice if that happens.
    assert_eq!(1880, core::mem::size_of::<reference_impl::Hasher>());
}

#[test]
fn test_counter_words() {
    let counter: u64 = (1 << 32) + 2;
    assert_eq!(crate::counter_low(counter), 2);
    assert_eq!(crate::counter_high(counter), 1);
}

#[test]
fn test_largest_power_of_two_leq() {
    let input_output = &[
        // The zero case is nonsensical, but it does work.
        (0, 1),
        (1, 1),
        (2, 2),
        (3, 2),
        (4, 4),
        (5, 4),
        (6, 4),
        (7, 4),
        (8, 8),
        // the largest possible usize
        (usize::MAX, (usize::MAX >> 1) + 1),
    ];
    for &(input, output) in input_output {
        assert_eq!(
            output,
            crate::largest_power_of_two_leq(input),
            "wrong output for n={}",
            input
        );
    }
}

#[test]
fn test_left_len() {
    let input_output = &[
        (CHUNK_LEN + 1, CHUNK_LEN),
        (2 * CHUNK_LEN - 1, CHUNK_LEN),
        (2 * CHUNK_LEN, CHUNK_LEN),
        (2 * CHUNK_LEN + 1, 2 * CHUNK_LEN),
        (4 * CHUNK_LEN - 1, 2 * CHUNK_LEN),
        (4 * CHUNK_LEN, 2 * CHUNK_LEN),
        (4 * CHUNK_LEN + 1, 4 * CHUNK_LEN),
    ];
    for &(input, output) in input_output {
        assert_eq!(crate::left_len(input), output);
    }
}

#[test]
fn test_compare_reference_impl() {
    const OUT: usize = 303; // more than 64, not a multiple of 4
    let mut input_buf = [0; TEST_CASES_MAX];
    paint_test_input(&mut input_buf);
    for &case in TEST_CASES {
        let input = &input_buf[..case];
        #[cfg(feature = "std")]
        dbg!(case);

        // regular
        {
            let mut reference_hasher = reference_impl::Hasher::new();
            reference_hasher.update(input);
            let mut expected_out = [0; OUT];
            reference_hasher.finalize(&mut expected_out);

            // all at once
            let test_out = crate::hash(input);
            assert_eq!(test_out, *array_ref!(expected_out, 0, 32));
            // incremental
            let mut hasher = crate::Hasher::new();
            hasher.update(input);
            assert_eq!(hasher.finalize(), *array_ref!(expected_out, 0, 32));
            assert_eq!(hasher.finalize(), test_out);
            // incremental (rayon)
            #[cfg(feature = "rayon")]
            {
                let mut hasher = crate::Hasher::new();
                hasher.update_rayon(input);
                assert_eq!(hasher.finalize(), *array_ref!(expected_out, 0, 32));
                assert_eq!(hasher.finalize(), test_out);
            }
            // xof
            let mut extended = [0; OUT];
            hasher.finalize_xof().fill(&mut extended);
            assert_eq!(extended, expected_out);
        }

        // keyed
        {
            let mut reference_hasher = reference_impl::Hasher::new_keyed(TEST_KEY);
            reference_hasher.update(input);
            let mut expected_out = [0; OUT];
            reference_hasher.finalize(&mut expected_out);

            // all at once
            let test_out = crate::keyed_hash(TEST_KEY, input);
            assert_eq!(test_out, *array_ref!(expected_out, 0, 32));
            // incremental
            let mut hasher = crate::Hasher::new_keyed(TEST_KEY);
            hasher.update(input);
            assert_eq!(hasher.finalize(), *array_ref!(expected_out, 0, 32));
            assert_eq!(hasher.finalize(), test_out);
            // incremental (rayon)
            #[cfg(feature = "rayon")]
            {
                let mut hasher = crate::Hasher::new_keyed(TEST_KEY);
                hasher.update_rayon(input);
                assert_eq!(hasher.finalize(), *array_ref!(expected_out, 0, 32));
                assert_eq!(hasher.finalize(), test_out);
            }
            // xof
            let mut extended = [0; OUT];
            hasher.finalize_xof().fill(&mut extended);
            assert_eq!(extended, expected_out);
        }

        // derive_key
        {
            let context = "BLAKE3 2019-12-27 16:13:59 example context (not the test vector one)";
            let mut reference_hasher = reference_impl::Hasher::new_derive_key(context);
            reference_hasher.update(input);
            let mut expected_out = [0; OUT];
            reference_hasher.finalize(&mut expected_out);

            // all at once
            let test_out = crate::derive_key(context, input);
            assert_eq!(test_out, expected_out[..32]);
            // incremental
            let mut hasher = crate::Hasher::new_derive_key(context);
            hasher.update(input);
            assert_eq!(hasher.finalize(), *array_ref!(expected_out, 0, 32));
            assert_eq!(hasher.finalize(), *array_ref!(test_out, 0, 32));
            // incremental (rayon)
            #[cfg(feature = "rayon")]
            {
                let mut hasher = crate::Hasher::new_derive_key(context);
                hasher.update_rayon(input);
                assert_eq!(hasher.finalize(), *array_ref!(expected_out, 0, 32));
                assert_eq!(hasher.finalize(), *array_ref!(test_out, 0, 32));
            }
            // xof
            let mut extended = [0; OUT];
            hasher.finalize_xof().fill(&mut extended);
            assert_eq!(extended, expected_out);
        }
    }
}

fn reference_hash(input: &[u8]) -> crate::Hash {
    let mut hasher = reference_impl::Hasher::new();
    hasher.update(input);
    let mut bytes = [0; 32];
    hasher.finalize(&mut bytes);
    bytes.into()
}

#[test]
fn test_compare_update_multiple() {
    // Don't use all the long test cases here, since that's unnecessarily slow
    // in debug mode.
    let mut short_test_cases = TEST_CASES;
    while *short_test_cases.last().unwrap() > 4 * CHUNK_LEN {
        short_test_cases = &short_test_cases[..short_test_cases.len() - 1];
    }
    assert_eq!(*short_test_cases.last().unwrap(), 4 * CHUNK_LEN);

    let mut input_buf = [0; 2 * TEST_CASES_MAX];
    paint_test_input(&mut input_buf);

    for &first_update in short_test_cases {
        #[cfg(feature = "std")]
        dbg!(first_update);
        let first_input = &input_buf[..first_update];
        let mut test_hasher = crate::Hasher::new();
        test_hasher.update(first_input);

        for &second_update in short_test_cases {
            #[cfg(feature = "std")]
            dbg!(second_update);
            let second_input = &input_buf[first_update..][..second_update];
            let total_input = &input_buf[..first_update + second_update];

            // Clone the hasher with first_update bytes already written, so
            // that the next iteration can reuse it.
            let mut test_hasher = test_hasher.clone();
            test_hasher.update(second_input);
            let expected = reference_hash(total_input);
            assert_eq!(expected, test_hasher.finalize());
        }
    }
}

#[test]
fn test_fuzz_hasher() {
    const INPUT_MAX: usize = 4 * CHUNK_LEN;
    let mut input_buf = [0; 3 * INPUT_MAX];
    paint_test_input(&mut input_buf);

    // Don't do too many iterations in debug mode, to keep the tests under a
    // second or so. CI should run tests in release mode also. Provide an
    // environment variable for specifying a larger number of fuzz iterations.
    let num_tests = if cfg!(debug_assertions) { 100 } else { 10_000 };

    // Use a fixed RNG seed for reproducibility.
    let mut rng = rand_chacha::ChaCha8Rng::from_seed([1; 32]);
    for _num_test in 0..num_tests {
        #[cfg(feature = "std")]
        dbg!(_num_test);
        let mut hasher = crate::Hasher::new();
        let mut total_input = 0;
        // For each test, write 3 inputs of random length.
        for _ in 0..3 {
            let input_len = rng.gen_range(0..(INPUT_MAX + 1));
            #[cfg(feature = "std")]
            dbg!(input_len);
            let input = &input_buf[total_input..][..input_len];
            hasher.update(input);
            total_input += input_len;
        }
        let expected = reference_hash(&input_buf[..total_input]);
        assert_eq!(expected, hasher.finalize());
    }
}

#[test]
fn test_xof_seek() {
    let mut out = [0; 533];
    let mut hasher = crate::Hasher::new();
    hasher.update(b"foo");
    hasher.finalize_xof().fill(&mut out);
    assert_eq!(hasher.finalize().as_bytes(), &out[0..32]);

    let mut reader = hasher.finalize_xof();
    reader.set_position(303);
    let mut out2 = [0; 102];
    reader.fill(&mut out2);
    assert_eq!(&out[303..][..102], &out2[..]);

    #[cfg(feature = "std")]
    {
        use std::io::prelude::*;
        let mut reader = hasher.finalize_xof();
        reader.seek(std::io::SeekFrom::Start(303)).unwrap();
        let mut out3 = Vec::new();
        reader.by_ref().take(102).read_to_end(&mut out3).unwrap();
        assert_eq!(&out[303..][..102], &out3[..]);

        assert_eq!(
            reader.seek(std::io::SeekFrom::Current(0)).unwrap(),
            303 + 102
        );
        reader.seek(std::io::SeekFrom::Current(-5)).unwrap();
        assert_eq!(
            reader.seek(std::io::SeekFrom::Current(0)).unwrap(),
            303 + 102 - 5
        );
        let mut out4 = [0; 17];
        assert_eq!(reader.read(&mut out4).unwrap(), 17);
        assert_eq!(&out[303 + 102 - 5..][..17], &out4[..]);
        assert_eq!(
            reader.seek(std::io::SeekFrom::Current(0)).unwrap(),
            303 + 102 - 5 + 17
        );
        assert!(reader.seek(std::io::SeekFrom::End(0)).is_err());
        assert!(reader.seek(std::io::SeekFrom::Current(-1000)).is_err());
    }
}

#[test]
fn test_msg_schedule_permutation() {
    let permutation = [2, 6, 3, 10, 7, 0, 4, 13, 1, 11, 12, 5, 9, 14, 15, 8];

    let mut generated = [[0; 16]; 7];
    generated[0] = [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15];

    for round in 1..7 {
        for i in 0..16 {
            generated[round][i] = generated[round - 1][permutation[i]];
        }
    }

    assert_eq!(generated, crate::MSG_SCHEDULE);
}

#[test]
fn test_reset() {
    let mut hasher = crate::Hasher::new();
    hasher.update(&[42; 3 * CHUNK_LEN + 7]);
    hasher.reset();
    hasher.update(&[42; CHUNK_LEN + 3]);
    assert_eq!(hasher.finalize(), crate::hash(&[42; CHUNK_LEN + 3]));

    let key = &[99; crate::KEY_LEN];
    let mut keyed_hasher = crate::Hasher::new_keyed(key);
    keyed_hasher.update(&[42; 3 * CHUNK_LEN + 7]);
    keyed_hasher.reset();
    keyed_hasher.update(&[42; CHUNK_LEN + 3]);
    assert_eq!(
        keyed_hasher.finalize(),
        crate::keyed_hash(key, &[42; CHUNK_LEN + 3]),
    );

    let context = "BLAKE3 2020-02-12 10:20:58 reset test";
    let mut kdf = crate::Hasher::new_derive_key(context);
    kdf.update(&[42; 3 * CHUNK_LEN + 7]);
    kdf.reset();
    kdf.update(&[42; CHUNK_LEN + 3]);
    let expected = crate::derive_key(context, &[42; CHUNK_LEN + 3]);
    assert_eq!(kdf.finalize(), expected);
}

#[test]
fn test_hex_encoding_decoding() {
    let digest_str = "04e0bb39f30b1a3feb89f536c93be15055482df748674b00d26e5a75777702e9";
    let mut hasher = crate::Hasher::new();
    hasher.update(b"foo");
    let digest = hasher.finalize();
    assert_eq!(digest.to_hex().as_str(), digest_str);
    #[cfg(feature = "std")]
    assert_eq!(digest.to_string(), digest_str);

    // Test round trip
    let digest = crate::Hash::from_hex(digest_str).unwrap();
    assert_eq!(digest.to_hex().as_str(), digest_str);

    // Test uppercase
    let digest = crate::Hash::from_hex(digest_str.to_uppercase()).unwrap();
    assert_eq!(digest.to_hex().as_str(), digest_str);

    // Test string parsing via FromStr
    let digest: crate::Hash = digest_str.parse().unwrap();
    assert_eq!(digest.to_hex().as_str(), digest_str);

    // Test errors
    let bad_len = "04e0bb39f30b1";
    let _result = crate::Hash::from_hex(bad_len).unwrap_err();
    #[cfg(feature = "std")]
    assert_eq!(_result.to_string(), "expected 64 hex bytes, received 13");

    let bad_char = "Z4e0bb39f30b1a3feb89f536c93be15055482df748674b00d26e5a75777702e9";
    let _result = crate::Hash::from_hex(bad_char).unwrap_err();
    #[cfg(feature = "std")]
    assert_eq!(_result.to_string(), "invalid hex character: 'Z'");

    let _result = crate::Hash::from_hex([128; 64]).unwrap_err();
    #[cfg(feature = "std")]
    assert_eq!(_result.to_string(), "invalid hex character: 0x80");
}

// This test is a mimized failure case for the Windows SSE2 bug described in
// https://github.com/BLAKE3-team/BLAKE3/issues/206.
//
// Before that issue was fixed, this test would fail on Windows in the following configuration:
//
//     cargo test --features=no_avx512,no_avx2,no_sse41 --release
//
// Bugs like this one (stomping on a caller's register) are very sensitive to the details of
// surrounding code, so it's not especially likely that this test will catch another bug (or even
// the same bug) in the future. Still, there's no harm in keeping it.
#[test]
fn test_issue_206_windows_sse2() {
    // This stupid loop has to be here to trigger the bug. I don't know why.
    for _ in &[0] {
        // The length 65 (two blocks) is significant. It doesn't repro with 64 (one block). It also
        // doesn't repro with an all-zero input.
        let input = &[0xff; 65];
        let expected_hash = [
            183, 235, 50, 217, 156, 24, 190, 219, 2, 216, 176, 255, 224, 53, 28, 95, 57, 148, 179,
            245, 162, 90, 37, 121, 0, 142, 219, 62, 234, 204, 225, 161,
        ];

        // This throwaway call has to be here to trigger the bug.
        crate::Hasher::new().update(input);

        // This assert fails when the bug is triggered.
        assert_eq!(crate::Hasher::new().update(input).finalize(), expected_hash);
    }
}

#[test]
fn test_hash_conversions() {
    let bytes1 = [42; 32];
    let hash1: crate::Hash = bytes1.into();
    let bytes2: [u8; 32] = hash1.into();
    assert_eq!(bytes1, bytes2);

    let bytes3 = *hash1.as_bytes();
    assert_eq!(bytes1, bytes3);

    let hash2 = crate::Hash::from_bytes(bytes1);
    assert_eq!(hash1, hash2);

    let hex = hash1.to_hex();
    let hash3 = crate::Hash::from_hex(hex.as_bytes()).unwrap();
    assert_eq!(hash1, hash3);
}

#[test]
const fn test_hash_const_conversions() {
    let bytes = [42; 32];
    let hash = crate::Hash::from_bytes(bytes);
    _ = hash.as_bytes();
}

#[cfg(feature = "zeroize")]
#[test]
fn test_zeroize() {
    use zeroize::Zeroize;

    let mut hash = crate::Hash([42; 32]);
    hash.zeroize();
    assert_eq!(hash.0, [0u8; 32]);

    let mut hasher = crate::Hasher {
        chunk_state: crate::ChunkState {
            cv: [42; 8],
            chunk_counter: 42,
            buf: [42; 64],
            buf_len: 42,
            blocks_compressed: 42,
            flags: 42,
            platform: crate::Platform::Portable,
        },
        key: [42; 8],
        cv_stack: [[42; 32]; { crate::MAX_DEPTH + 1 }].into(),
    };
    hasher.zeroize();
    assert_eq!(hasher.chunk_state.cv, [0; 8]);
    assert_eq!(hasher.chunk_state.chunk_counter, 0);
    assert_eq!(hasher.chunk_state.buf, [0; 64]);
    assert_eq!(hasher.chunk_state.buf_len, 0);
    assert_eq!(hasher.chunk_state.blocks_compressed, 0);
    assert_eq!(hasher.chunk_state.flags, 0);
    assert!(matches!(hasher.chunk_state.platform, crate::Platform::Portable));
    assert_eq!(hasher.key, [0; 8]);
    assert_eq!(&*hasher.cv_stack, &[[0u8; 32]; 0]);


    let mut output_reader = crate::OutputReader {
        inner: crate::Output {
            input_chaining_value: [42; 8],
            block: [42; 64],
            counter: 42,
            block_len: 42,
            flags: 42,
            platform: crate::Platform::Portable,
        },
        position_within_block: 42,
    };


    output_reader.zeroize();
    assert_eq!(output_reader.inner.input_chaining_value, [0; 8]);
    assert_eq!(output_reader.inner.block, [0; 64]);
    assert_eq!(output_reader.inner.counter, 0);
    assert_eq!(output_reader.inner.block_len, 0);
    assert_eq!(output_reader.inner.flags, 0);
    assert!(matches!(output_reader.inner.platform, crate::Platform::Portable));
    assert_eq!(output_reader.position_within_block, 0);

}