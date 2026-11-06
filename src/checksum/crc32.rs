//! CRC-32 (ISO-HDLC, the gzip and zip checksum), slicing-by-16.

/// The reflected CRC-32 polynomial (x^32 + x^26 + ... + 1).
const POLY: u32 = 0xEDB8_8320;

/// Sixteen 256-entry tables: `TABLES[0]` is the classic byte table, and
/// `TABLES[k][b]` is the CRC of byte `b` followed by `k` zero bytes. Built at
/// compile time (16 KiB of read-only data).
static TABLES: [[u32; 256]; 16] = build_tables();

const fn build_tables() -> [[u32; 256]; 16] {
    let mut tables = [[0u32; 256]; 16];
    let mut i = 0;
    while i < 256 {
        let mut crc = i as u32;
        let mut bit = 0;
        while bit < 8 {
            crc = if crc & 1 != 0 {
                (crc >> 1) ^ POLY
            } else {
                crc >> 1
            };
            bit += 1;
        }
        tables[0][i] = crc;
        i += 1;
    }
    let mut k = 1;
    while k < 16 {
        let mut i = 0;
        while i < 256 {
            let prev = tables[k - 1][i];
            tables[k][i] = (prev >> 8) ^ tables[0][(prev & 0xFF) as usize];
            i += 1;
        }
        k += 1;
    }
    tables
}

/// Advances a raw (pre- and post-inverted by the caller) CRC over `data`.
fn update(mut crc: u32, data: &[u8]) -> u32 {
    let t = &TABLES;
    let mut chunks = data.chunks_exact(16);
    for c in &mut chunks {
        let a = crc ^ u32::from_le_bytes([c[0], c[1], c[2], c[3]]);
        crc = t[15][(a & 0xFF) as usize]
            ^ t[14][((a >> 8) & 0xFF) as usize]
            ^ t[13][((a >> 16) & 0xFF) as usize]
            ^ t[12][(a >> 24) as usize]
            ^ t[11][c[4] as usize]
            ^ t[10][c[5] as usize]
            ^ t[9][c[6] as usize]
            ^ t[8][c[7] as usize]
            ^ t[7][c[8] as usize]
            ^ t[6][c[9] as usize]
            ^ t[5][c[10] as usize]
            ^ t[4][c[11] as usize]
            ^ t[3][c[12] as usize]
            ^ t[2][c[13] as usize]
            ^ t[1][c[14] as usize]
            ^ t[0][c[15] as usize];
    }
    for &b in chunks.remainder() {
        crc = (crc >> 8) ^ t[0][((crc ^ u32::from(b)) & 0xFF) as usize];
    }
    crc
}

/// Computes the CRC-32 of `data` in one call.
///
/// This is the checksum gzip (RFC 1952) and zip use: polynomial `0x04C11DB7`
/// reflected, initial value and final XOR `0xFFFFFFFF`.
///
/// # Examples
///
/// ```
/// use compress_lib::checksum::crc32;
///
/// assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
/// assert_eq!(crc32(b""), 0);
/// ```
#[must_use]
pub fn crc32(data: &[u8]) -> u32 {
    !update(!0, data)
}

/// A CRC-32 computed over data that arrives in pieces.
///
/// The result does not depend on how the data was split.
///
/// # Examples
///
/// ```
/// use compress_lib::checksum::{Crc32, crc32};
///
/// let mut crc = Crc32::new();
/// crc.update(b"1234");
/// crc.update(b"56789");
/// assert_eq!(crc.finish(), crc32(b"123456789"));
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Crc32 {
    /// The running value, inverted (the register as the algorithm keeps it).
    state: u32,
    /// Bytes seen, for [`Crc32::len`] and combining.
    len: u64,
}

impl Crc32 {
    /// A CRC-32 over no data yet.
    ///
    /// # Examples
    ///
    /// ```
    /// assert_eq!(compress_lib::checksum::Crc32::new().finish(), 0);
    /// ```
    #[must_use]
    pub const fn new() -> Self {
        Self { state: !0, len: 0 }
    }

    /// Continues from a CRC-32 value already computed over `len` bytes (for
    /// example, a stored checksum that more data is appended to).
    ///
    /// # Examples
    ///
    /// ```
    /// use compress_lib::checksum::{Crc32, crc32};
    ///
    /// let mut crc = Crc32::from_value(crc32(b"1234"), 4);
    /// crc.update(b"56789");
    /// assert_eq!(crc.finish(), crc32(b"123456789"));
    /// ```
    #[must_use]
    pub const fn from_value(crc: u32, len: u64) -> Self {
        Self { state: !crc, len }
    }

    /// Feeds more data.
    ///
    /// # Examples
    ///
    /// ```
    /// let mut crc = compress_lib::checksum::Crc32::new();
    /// crc.update(b"abc");
    /// assert_eq!(crc.finish(), 0x3524_41C2);
    /// ```
    pub fn update(&mut self, data: &[u8]) {
        self.state = update(self.state, data);
        self.len = self.len.wrapping_add(data.len() as u64);
    }

    /// The CRC-32 of everything fed so far. Does not reset; more data may
    /// follow.
    ///
    /// # Examples
    ///
    /// ```
    /// let mut crc = compress_lib::checksum::Crc32::new();
    /// crc.update(b"a");
    /// assert_eq!(crc.finish(), 0xE8B7_BE43);
    /// ```
    #[must_use]
    pub const fn finish(&self) -> u32 {
        !self.state
    }

    /// The number of bytes fed so far (wrapping at 2^64).
    ///
    /// # Examples
    ///
    /// ```
    /// let mut crc = compress_lib::checksum::Crc32::new();
    /// crc.update(b"abc");
    /// assert_eq!(crc.len(), 3);
    /// ```
    #[must_use]
    pub const fn len(&self) -> u64 {
        self.len
    }

    /// `true` when no data has been fed.
    ///
    /// # Examples
    ///
    /// ```
    /// assert!(compress_lib::checksum::Crc32::new().is_empty());
    /// ```
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Appends the checksum of a second piece of data computed separately,
    /// as if its bytes had been fed here.
    ///
    /// # Examples
    ///
    /// ```
    /// use compress_lib::checksum::{Crc32, crc32};
    ///
    /// let mut left = Crc32::new();
    /// left.update(b"hello ");
    /// let mut right = Crc32::new();
    /// right.update(b"world");
    /// left.combine(&right);
    /// assert_eq!(left.finish(), crc32(b"hello world"));
    /// ```
    pub fn combine(&mut self, other: &Crc32) {
        let combined = crc32_combine(self.finish(), other.finish(), other.len);
        self.state = !combined;
        self.len = self.len.wrapping_add(other.len);
    }
}

impl Default for Crc32 {
    fn default() -> Self {
        Self::new()
    }
}

/// Multiplies two polynomials modulo the CRC polynomial (reflected bit
/// order). The loop runs at most 32 times.
const fn multiply_mod(a: u32, mut b: u32) -> u32 {
    let mut product = 0;
    let mut mask: u32 = 1 << 31;
    while mask != 0 {
        if a & mask != 0 {
            product ^= b;
        }
        mask >>= 1;
        b = if b & 1 != 0 { (b >> 1) ^ POLY } else { b >> 1 };
    }
    product
}

/// `POWERS[k]` is x^(2^k) modulo the polynomial.
static POWERS: [u32; 67] = build_powers();

const fn build_powers() -> [u32; 67] {
    let mut powers = [0u32; 67];
    let mut p: u32 = 1 << 30; // x^1
    powers[0] = p;
    let mut k = 1;
    while k < 67 {
        p = multiply_mod(p, p);
        powers[k] = p;
        k += 1;
    }
    powers
}

/// x^(8 * bytes) modulo the polynomial: the effect of `bytes` zero bytes.
fn shift_bytes(bytes: u64) -> u32 {
    let mut p: u32 = 1 << 31; // x^0
    let mut n = bytes;
    // Multiplying by 8 is three squarings, so start three entries into the
    // table; a 64-bit count then needs entries 3 to 66.
    let mut k = 3;
    while n != 0 && k < POWERS.len() {
        if n & 1 != 0 {
            p = multiply_mod(POWERS[k], p);
        }
        n >>= 1;
        k += 1;
    }
    p
}

/// Combines `crc1` (over data A) and `crc2` (over data B, `len2` bytes long)
/// into the CRC-32 of A followed by B, in O(log len2).
///
/// # Examples
///
/// ```
/// use compress_lib::checksum::{crc32, crc32_combine};
///
/// let ab = crc32_combine(crc32(b"hello "), crc32(b"world"), 5);
/// assert_eq!(ab, crc32(b"hello world"));
/// ```
#[must_use]
pub fn crc32_combine(crc1: u32, crc2: u32, len2: u64) -> u32 {
    multiply_mod(shift_bytes(len2), crc1) ^ crc2
}

#[cfg(test)]
mod tests {
    use alloc::vec::Vec;

    use proptest::prelude::*;

    use super::*;

    /// Bit-at-a-time reference implementation.
    fn reference(data: &[u8]) -> u32 {
        let mut crc = !0u32;
        for &b in data {
            crc ^= u32::from(b);
            for _ in 0..8 {
                crc = if crc & 1 != 0 {
                    (crc >> 1) ^ POLY
                } else {
                    crc >> 1
                };
            }
        }
        !crc
    }

    #[test]
    fn test_crc32_check_value_matches_catalogue() {
        // CRC-32/ISO-HDLC check value from the CRC catalogue.
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
    }

    #[test]
    fn test_crc32_known_strings_match() {
        assert_eq!(crc32(b""), 0);
        assert_eq!(crc32(b"a"), 0xE8B7_BE43);
        assert_eq!(crc32(b"abc"), 0x3524_41C2);
        assert_eq!(
            crc32(b"The quick brown fox jumps over the lazy dog"),
            0x414F_A339
        );
        assert_eq!(crc32(&[0u8; 32]), 0x190A_55AD);
        assert_eq!(crc32(&[0xFFu8; 32]), 0xFF6C_AB0B);
    }

    #[test]
    fn test_crc32_every_length_to_100_matches_reference() {
        let data: Vec<u8> = (0..100u32).map(|i| (i * 31 + 7) as u8).collect();
        for n in 0..=data.len() {
            assert_eq!(crc32(&data[..n]), reference(&data[..n]), "length {n}");
        }
    }

    #[test]
    fn test_crc32_from_value_continues() {
        let mut crc = Crc32::from_value(crc32(b"12345"), 5);
        crc.update(b"6789");
        assert_eq!(crc.finish(), 0xCBF4_3926);
        assert_eq!(crc.len(), 9);
        assert!(!crc.is_empty());
        assert_eq!(Crc32::default(), Crc32::new());
    }

    #[test]
    fn test_crc32_combine_empty_second_is_identity() {
        assert_eq!(crc32_combine(0x1234_5678, 0, 0), 0x1234_5678);
        let mut a = Crc32::new();
        a.update(b"x");
        a.combine(&Crc32::new());
        assert_eq!(a.finish(), crc32(b"x"));
    }

    #[test]
    fn test_crc32_combine_huge_length_terminates() {
        // Not checked against data (2^64 bytes); this proves the loop ends and
        // stays deterministic at the extreme.
        let a = crc32_combine(1, 2, u64::MAX);
        assert_eq!(a, crc32_combine(1, 2, u64::MAX));
    }

    proptest! {
        #[test]
        fn prop_crc32_matches_reference(data in proptest::collection::vec(any::<u8>(), 0..2048)) {
            prop_assert_eq!(crc32(&data), reference(&data));
        }

        #[test]
        fn prop_crc32_split_invariant(data in proptest::collection::vec(any::<u8>(), 0..2048), cut in any::<usize>()) {
            let cut = if data.is_empty() { 0 } else { cut % (data.len() + 1) };
            let mut crc = Crc32::new();
            crc.update(&data[..cut]);
            crc.update(&data[cut..]);
            prop_assert_eq!(crc.finish(), crc32(&data));
        }

        #[test]
        fn prop_crc32_combine_matches_concatenation(a in proptest::collection::vec(any::<u8>(), 0..512), b in proptest::collection::vec(any::<u8>(), 0..512)) {
            let mut whole = a.clone();
            whole.extend_from_slice(&b);
            prop_assert_eq!(crc32_combine(crc32(&a), crc32(&b), b.len() as u64), crc32(&whole));
        }
    }
}
