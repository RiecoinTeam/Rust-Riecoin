// SPDX-License-Identifier: CC0-1.0

//! SHA256 implementation.

mod crypto;

use core::{cmp, convert, fmt};

use crate::{incomplete_block_len, sha256d, HashEngine as _};
#[cfg(doc)]
use crate::{sha256t, sha256t_tag};

crate::internal_macros::general_hash_type! {
    256,
    false,
    "Output of the SHA256 hash function."
}

#[cfg(not(hashes_fuzz))]
fn from_engine(mut e: HashEngine) -> Hash {
    // pad buffer with a single 1-bit then all 0s, until there are exactly 8 bytes remaining
    let n_bytes_hashed = e.bytes_hashed;

    let zeroes = [0; BLOCK_SIZE - 8];
    e.input(&[0x80]);
    if incomplete_block_len(&e) > zeroes.len() {
        e.input(&zeroes);
    }
    let pad_length = zeroes.len() - incomplete_block_len(&e);
    e.input(&zeroes[..pad_length]);
    debug_assert_eq!(incomplete_block_len(&e), zeroes.len());

    e.input(&(8 * n_bytes_hashed).to_be_bytes());
    debug_assert_eq!(incomplete_block_len(&e), 0);

    Hash(e.midstate_unchecked().bytes)
}

#[cfg(hashes_fuzz)]
fn from_engine(e: HashEngine) -> Hash {
    let mut hash = e.midstate_unchecked().bytes;
    if hash == [0; 32] {
        // Assume sha256 is secure and never generate 0-hashes (which represent invalid
        // secp256k1 secret keys, causing downstream application breakage).
        hash[0] = 1;
    }
    Hash(hash)
}

const BLOCK_SIZE: usize = 64;

/// Engine to compute SHA256 hash function.
#[derive(Clone)]
pub struct HashEngine {
    buffer: [u8; BLOCK_SIZE],
    h: [u32; 8],
    bytes_hashed: u64,
}

impl HashEngine {
    /// Constructs a new SHA256 hash engine.
    pub const fn new() -> Self {
        Self {
            h: [
                0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab,
                0x5be0cd19,
            ],
            bytes_hashed: 0,
            buffer: [0; BLOCK_SIZE],
        }
    }

    /// Constructs a new [`HashEngine`] from a [`Midstate`].
    ///
    /// Please see docs on [`Midstate`] before using this function.
    pub fn from_midstate(midstate: Midstate) -> HashEngine {
        let mut ret = [0; 8];
        for (ret_val, midstate_bytes) in ret.iter_mut().zip(midstate.as_ref().chunks_exact(4)) {
            *ret_val = u32::from_be_bytes(midstate_bytes.try_into().expect("4 byte slice"));
        }

        HashEngine { buffer: [0; BLOCK_SIZE], h: ret, bytes_hashed: midstate.bytes_hashed }
    }

    /// Returns `true` if the midstate can be extracted from this engine.
    ///
    /// The midstate can only be extracted if the number of bytes input into
    /// the hash engine is a multiple of 64. See caveat on [`Self::midstate`].
    ///
    /// Please see docs on [`Midstate`] before using this function.
    pub const fn can_extract_midstate(&self) -> bool { self.bytes_hashed % 64 == 0 }

    /// Outputs the midstate of the hash engine.
    ///
    /// Please see docs on [`Midstate`] before using this function.
    pub fn midstate(&self) -> Result<Midstate, MidstateError> {
        if !self.can_extract_midstate() {
            return Err(MidstateError { invalid_n_bytes_hashed: self.bytes_hashed });
        }
        Ok(self.midstate_unchecked())
    }

    // Does not check that `HashEngine::can_extract_midstate`.
    #[cfg(not(hashes_fuzz))]
    fn midstate_unchecked(&self) -> Midstate {
        let mut ret = [0; 32];
        for (val, ret_bytes) in self.h.iter().zip(ret.chunks_exact_mut(4)) {
            ret_bytes.copy_from_slice(&val.to_be_bytes());
        }
        Midstate { bytes: ret, bytes_hashed: self.bytes_hashed }
    }

    // Does not check that `HashEngine::can_extract_midstate`.
    #[cfg(hashes_fuzz)]
    fn midstate_unchecked(&self) -> Midstate {
        let mut ret = [0; 32];
        ret.copy_from_slice(&self.buffer[..32]);
        Midstate { bytes: ret, bytes_hashed: self.bytes_hashed }
    }
}

impl Default for HashEngine {
    fn default() -> Self { Self::new() }
}

impl crate::HashEngine for HashEngine {
    const BLOCK_SIZE: usize = 64;

    fn n_bytes_hashed(&self) -> u64 { self.bytes_hashed }

    crate::internal_macros::engine_input_impl!();
}

impl Hash {
    /// Iterate the sha256 algorithm to turn a sha256 hash into a sha256d hash
    #[must_use]
    pub fn hash_again(&self) -> sha256d::Hash {
        crate::Hash::from_byte_array(<Self as crate::GeneralHash>::hash(&self.0).0)
    }

    /// Computes hash from `bytes` in `const` context.
    ///
    /// Warning: this function is inefficient. It should be only used in `const` context.
    #[deprecated(since = "0.15.0", note = "use `Self::hash_unoptimized` instead")]
    pub const fn const_hash(bytes: &[u8]) -> Self { Hash::hash_unoptimized(bytes) }

    /// Computes hash from `bytes` in `const` context.
    ///
    /// Warning: this function is inefficient. It should be only used in `const` context.
    pub const fn hash_unoptimized(bytes: &[u8]) -> Self {
        Hash(Midstate::compute_midstate_unoptimized(bytes, true).bytes)
    }
}

/// Unfinalized output of the SHA256 hash function.
///
/// The `Midstate` type is obscure and specialized and should not be used unless you are sure of
/// what you are doing.
///
/// It represents "partially hashed data" but does not itself have properties of cryptographic
/// hashes. For example, when (ab)used as hashes, midstates are vulnerable to trivial
/// length-extension attacks. They are typically used to optimize the computation of full hashes.
/// For example, when implementing BIP-340 tagged hashes, which always begin by hashing the same
/// fixed 64-byte prefix, it makes sense to hash the prefix once, store the midstate as a constant,
/// and hash any future data starting from the constant rather than from a fresh hash engine.
///
/// For BIP-340 support we provide the [`sha256t`] module, and the [`sha256t_tag`] macro which will
/// create the midstate for you in const context.
#[derive(Copy, Clone, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Midstate {
    /// Raw bytes of the midstate i.e., the already-hashed contents of the hash engine.
    bytes: [u8; 32],
    /// Number of bytes hashed to achieve this midstate.
    // INVARIANT must always be a multiple of 64.
    bytes_hashed: u64,
}

impl Midstate {
    /// Construct a new [`Midstate`] from the `state` and the `bytes_hashed` to get to that state.
    ///
    /// # Panics
    ///
    /// Panics if `bytes_hashed` is not a multiple of 64.
    pub const fn new(state: [u8; 32], bytes_hashed: u64) -> Self {
        if bytes_hashed % 64 != 0 {
            panic!("bytes hashed is not a multiple of 64");
        }

        Midstate { bytes: state, bytes_hashed }
    }

    /// Deconstructs the [`Midstate`], returning the underlying byte array and number of bytes hashed.
    pub const fn as_parts(&self) -> (&[u8; 32], u64) { (&self.bytes, self.bytes_hashed) }

    /// Deconstructs the [`Midstate`], returning the underlying byte array and number of bytes hashed.
    pub const fn to_parts(self) -> ([u8; 32], u64) { (self.bytes, self.bytes_hashed) }

    /// Constructs a new midstate for tagged hashes.
    ///
    /// Warning: this function is inefficient. It should be only used in `const` context.
    ///
    /// Computes non-finalized hash of `sha256(tag) || sha256(tag)` for use in [`sha256t`]. It's
    /// provided for use with [`sha256t`].
    #[must_use]
    pub const fn hash_tag(tag: &[u8]) -> Self {
        let hash = Hash::hash_unoptimized(tag);
        let mut buf = [0u8; 64];
        let mut i = 0usize;
        while i < buf.len() {
            buf[i] = hash.0[i % hash.0.len()];
            i += 1;
        }
        Self::compute_midstate_unoptimized(&buf, false)
    }
}

impl fmt::Debug for Midstate {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        struct Encoder<'a> {
            bytes: &'a [u8; 32],
        }
        impl fmt::Debug for Encoder<'_> {
            fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result { crate::debug_hex(self.bytes, f) }
        }

        f.debug_struct("Midstate")
            .field("bytes", &Encoder { bytes: &self.bytes })
            .field("length", &self.bytes_hashed)
            .finish()
    }
}

impl convert::AsRef<[u8]> for Midstate {
    fn as_ref(&self) -> &[u8] { &self.bytes }
}

/// `Midstate` invariant violated (not a multiple of 64).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MidstateError {
    /// The invalid number of bytes hashed.
    invalid_n_bytes_hashed: u64,
}

impl fmt::Display for MidstateError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(
            f,
            "invalid number of bytes hashed {} (should have been a multiple of 64)",
            self.invalid_n_bytes_hashed
        )
    }
}

#[cfg(feature = "std")]
impl std::error::Error for MidstateError {}

#[cfg(test)]
mod tests {
    use core::array;

    use super::*;
    use crate::{sha256, HashEngine};

    #[test]
    #[cfg(feature = "alloc")]
    fn test() {
        use alloc::string::ToString;

        #[derive(Clone)]
        struct Test {
            input: &'static str,
            output: [u8; 32],
            output_str: &'static str,
        }

        #[rustfmt::skip]
        let tests = [
            // Examples from wikipedia
            Test {
                input: "",
                output: [
                    0xe3, 0xb0, 0xc4, 0x42, 0x98, 0xfc, 0x1c, 0x14,
                    0x9a, 0xfb, 0xf4, 0xc8, 0x99, 0x6f, 0xb9, 0x24,
                    0x27, 0xae, 0x41, 0xe4, 0x64, 0x9b, 0x93, 0x4c,
                    0xa4, 0x95, 0x99, 0x1b, 0x78, 0x52, 0xb8, 0x55,
                ],
                output_str: "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
            },
            Test {
                input: "The quick brown fox jumps over the lazy dog",
                output: [
                    0xd7, 0xa8, 0xfb, 0xb3, 0x07, 0xd7, 0x80, 0x94,
                    0x69, 0xca, 0x9a, 0xbc, 0xb0, 0x08, 0x2e, 0x4f,
                    0x8d, 0x56, 0x51, 0xe4, 0x6d, 0x3c, 0xdb, 0x76,
                    0x2d, 0x02, 0xd0, 0xbf, 0x37, 0xc9, 0xe5, 0x92,
                ],
                output_str: "d7a8fbb307d7809469ca9abcb0082e4f8d5651e46d3cdb762d02d0bf37c9e592",
            },
            Test {
                input: "The quick brown fox jumps over the lazy dog.",
                output: [
                    0xef, 0x53, 0x7f, 0x25, 0xc8, 0x95, 0xbf, 0xa7,
                    0x82, 0x52, 0x65, 0x29, 0xa9, 0xb6, 0x3d, 0x97,
                    0xaa, 0x63, 0x15, 0x64, 0xd5, 0xd7, 0x89, 0xc2,
                    0xb7, 0x65, 0x44, 0x8c, 0x86, 0x35, 0xfb, 0x6c,
                ],
                output_str: "ef537f25c895bfa782526529a9b63d97aa631564d5d789c2b765448c8635fb6c",
            },
        ];

        for test in tests {
            // Hash through high-level API, check hex encoding/decoding
            let hash = sha256::Hash::hash(test.input.as_bytes());
            assert_eq!(hash, test.output_str.parse::<sha256::Hash>().expect("parse hex"));
            assert_eq!(hash.as_byte_array(), &test.output);
            assert_eq!(hash.to_string(), test.output_str);

            // Hash through engine, checking that we can input byte by byte
            let mut engine = sha256::Hash::engine();
            for ch in test.input.as_bytes() {
                engine.input(&[*ch]);
            }
            let manual_hash = sha256::Hash::from_engine(engine);
            assert_eq!(hash, manual_hash);
            assert_eq!(hash.to_byte_array(), test.output);
        }
    }

    #[test]
    #[cfg(feature = "alloc")]
    fn fmt_roundtrips() {
        use alloc::format;

        let hash = sha256::Hash::hash(b"some arbitrary bytes");
        let hex = format!("{}", hash);
        let rinsed = hex.parse::<sha256::Hash>().expect("failed to parse hex");
        assert_eq!(rinsed, hash)
    }

    #[test]
    #[rustfmt::skip]
    pub(crate) fn midstate() {
        // Test vector obtained by doing an asset issuance on Elements
        let mut engine = sha256::Hash::engine();
        // sha256dhash of outpoint
        // 73828cbc65fd68ab78dc86992b76ae50ae2bf8ceedbe8de0483172f0886219f7:0
        engine.input(&[
            0x9d, 0xd0, 0x1b, 0x56, 0xb1, 0x56, 0x45, 0x14,
            0x3e, 0xad, 0x15, 0x8d, 0xec, 0x19, 0xf8, 0xce,
            0xa9, 0x0b, 0xd0, 0xa9, 0xb2, 0xf8, 0x1d, 0x21,
            0xff, 0xa3, 0xa4, 0xc6, 0x44, 0x81, 0xd4, 0x1c,
        ]);
        // 32 bytes of zeroes representing "new asset"
        engine.input(&[0; 32]);

        // RPC output
        static WANT: Midstate = sha256::Midstate::new([
            0x0b, 0xcf, 0xe0, 0xe5, 0x4e, 0x6c, 0xc7, 0xd3,
            0x4f, 0x4f, 0x7c, 0x1d, 0xf0, 0xb0, 0xf5, 0x03,
            0xf2, 0xf7, 0x12, 0x91, 0x2a, 0x06, 0x05, 0xb4,
            0x14, 0xed, 0x33, 0x7f, 0x7f, 0x03, 0x2e, 0x03,
        ], 64);

        assert_eq!(
            engine.midstate().expect("total_bytes_hashed is valid"),
            WANT,
        );
    }

    #[test]
    fn engine_with_state() {
        let mut engine = sha256::Hash::engine();
        let midstate_engine = sha256::HashEngine::from_midstate(engine.midstate_unchecked());
        // Fresh engine and engine initialized with fresh state should have same state
        assert_eq!(engine.h, midstate_engine.h);

        // Midstate changes after writing 64 bytes
        engine.input(&[1; 63]);
        assert_eq!(engine.h, midstate_engine.h);
        engine.input(&[2; 1]);
        assert_ne!(engine.h, midstate_engine.h);

        // Initializing an engine with midstate from another engine should result in
        // both engines producing the same hashes
        let data_vec: &[&[u8]] = &[&[3u8; 1], &[4u8; 63], &[5u8; 65], &[6u8; 66]];
        for data in data_vec {
            let mut engine = engine.clone();
            let mut midstate_engine =
                sha256::HashEngine::from_midstate(engine.midstate_unchecked());
            assert_eq!(engine.h, midstate_engine.h);
            assert_eq!(engine.bytes_hashed, midstate_engine.bytes_hashed);
            engine.input(data);
            midstate_engine.input(data);
            assert_eq!(engine.h, midstate_engine.h);
            let hash1 = sha256::Hash::from_engine(engine);
            let hash2 = sha256::Hash::from_engine(midstate_engine);
            assert_eq!(hash1, hash2);
        }

        // Test that a specific midstate results in a specific hash. Midstate was
        // obtained by applying sha256 to sha256("MuSig coefficient")||sha256("MuSig
        // coefficient").
        #[rustfmt::skip]
        static MIDSTATE: [u8; 32] = [
            0x0f, 0xd0, 0x69, 0x0c, 0xfe, 0xfe, 0xae, 0x97,
            0x99, 0x6e, 0xac, 0x7f, 0x5c, 0x30, 0xd8, 0x64,
            0x8c, 0x4a, 0x05, 0x73, 0xac, 0xa1, 0xa2, 0x2f,
            0x6f, 0x43, 0xb8, 0x01, 0x85, 0xce, 0x27, 0xcd,
        ];
        #[rustfmt::skip]
        static HASH_EXPECTED: [u8; 32] = [
            0x18, 0x84, 0xe4, 0x72, 0x40, 0x4e, 0xf4, 0x5a,
            0xb4, 0x9c, 0x4e, 0xa4, 0x9a, 0xe6, 0x23, 0xa8,
            0x88, 0x52, 0x7f, 0x7d, 0x8a, 0x06, 0x94, 0x20,
            0x8f, 0xf1, 0xf7, 0xa9, 0xd5, 0x69, 0x09, 0x59,
        ];
        let midstate_engine =
            sha256::HashEngine::from_midstate(sha256::Midstate::new(MIDSTATE, 64));
        let hash = sha256::Hash::from_engine(midstate_engine);
        assert_eq!(hash, sha256::Hash(HASH_EXPECTED));
    }

    #[test]
    fn hash_unoptimized() {
        let bytes: [u8; 256] = array::from_fn(|i| i as u8);

        for i in 0..=256 {
            let bytes = &bytes[0..i];
            assert_eq!(
                Hash::hash(bytes),
                Hash::hash_unoptimized(bytes),
                "hashes don't match for n_bytes_hashed {}",
                i + 1
            );
        }
    }

    // The midstate of an empty hash engine tagged with "TapLeaf".
    const TAP_LEAF_MIDSTATE: Midstate = Midstate::new(
        [
            156, 224, 228, 230, 124, 17, 108, 57, 56, 179, 202, 242, 195, 15, 80, 137, 211, 243,
            147, 108, 71, 99, 110, 96, 125, 179, 62, 234, 221, 198, 240, 201,
        ],
        64,
    );

    #[test]
    fn const_midstate() { assert_eq!(Midstate::hash_tag(b"TapLeaf"), TAP_LEAF_MIDSTATE,) }

    #[test]
    #[cfg(feature = "alloc")]
    fn regression_midstate_debug_format() {
        use alloc::format;

        let want = "Midstate { bytes: 9ce0e4e67c116c3938b3caf2c30f5089d3f3936c47636e607db33eeaddc6f0c9, length: 64 }";
        let got = format!("{:?}", TAP_LEAF_MIDSTATE);
        assert_eq!(got, want);
    }

    #[test]
    #[cfg(feature = "serde")]
    fn sha256_serde() {
        use serde_test::{assert_tokens, Configure, Token};

        #[rustfmt::skip]
        static HASH_BYTES: [u8; 32] = [
            0xef, 0x53, 0x7f, 0x25, 0xc8, 0x95, 0xbf, 0xa7,
            0x82, 0x52, 0x65, 0x29, 0xa9, 0xb6, 0x3d, 0x97,
            0xaa, 0x63, 0x15, 0x64, 0xd5, 0xd7, 0x89, 0xc2,
            0xb7, 0x65, 0x44, 0x8c, 0x86, 0x35, 0xfb, 0x6c,
        ];

        let hash = sha256::Hash::from_slice(&HASH_BYTES).expect("right number of bytes");
        assert_tokens(&hash.compact(), &[Token::BorrowedBytes(&HASH_BYTES[..])]);
        assert_tokens(
            &hash.readable(),
            &[Token::Str("ef537f25c895bfa782526529a9b63d97aa631564d5d789c2b765448c8635fb6c")],
        );
    }

    #[cfg(target_arch = "wasm32")]
    mod wasm_tests {
        use super::*;
        #[test]
        #[wasm_bindgen_test::wasm_bindgen_test]
        fn sha256_tests() {
            test();
            midstate();
            engine_with_state();
        }
    }
}

#[cfg(bench)]
mod benches {
    use test::Bencher;

    use crate::{sha256, Hash, HashEngine};

    #[bench]
    pub fn sha256_10(bh: &mut Bencher) {
        let mut engine = sha256::Hash::engine();
        let bytes = [1u8; 10];
        bh.iter(|| {
            engine.input(&bytes);
        });
        bh.bytes = bytes.len() as u64;
    }

    #[bench]
    pub fn sha256_1k(bh: &mut Bencher) {
        let mut engine = sha256::Hash::engine();
        let bytes = [1u8; 1024];
        bh.iter(|| {
            engine.input(&bytes);
        });
        bh.bytes = bytes.len() as u64;
    }

    #[bench]
    pub fn sha256_64k(bh: &mut Bencher) {
        let mut engine = sha256::Hash::engine();
        let bytes = [1u8; 65536];
        bh.iter(|| {
            engine.input(&bytes);
        });
        bh.bytes = bytes.len() as u64;
    }
}
