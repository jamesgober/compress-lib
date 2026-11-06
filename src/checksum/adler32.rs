//! Adler-32 (RFC 1950), four lanes with deferred modulo.

/// The largest prime below 2^16.
const MOD: u32 = 65_521;

/// The most bytes per lane before the sums must be reduced: the largest `n`
/// with `255 * n * (n + 1) / 2 + (n + 1) * (MOD - 1) < 2^32` (zlib's NMAX).
const NMAX: usize = 5_552;

/// Lanes processed side by side; the compiler keeps them in one vector
/// register.
const LANES: usize = 4;

/// Advances `(a, b)` over `data`.
fn update(a: u32, b: u32, data: &[u8]) -> (u32, u32) {
    let mut a = a;
    let mut b = b;
    let split = data.len() - data.len() % LANES;
    let (body, tail) = data.split_at(split);

    let mut lane_a = [0u32; LANES];
    let mut lane_b = [0u32; LANES];
    for block in body.chunks(NMAX * LANES) {
        for group in block.chunks_exact(LANES) {
            for i in 0..LANES {
                lane_a[i] += u32::from(group[i]);
                lane_b[i] += lane_a[i];
            }
        }
        // `a` stays the value before this run of lanes; every byte of the
        // block adds it once to `b`. `block.len() * a` fits: at most
        // 22,208 * 65,520.
        b += (block.len() as u32) * a;
        for i in 0..LANES {
            lane_a[i] %= MOD;
            lane_b[i] %= MOD;
        }
        b %= MOD;
    }

    // Fold the lanes: byte `4k + j` of the run contributes `(m - k)` times
    // to lane `j` of `b`, and should contribute `(4m - 4k - j)` times.
    for i in 0..LANES {
        b += LANES as u32 * lane_b[i] + (MOD - lane_a[i]) * i as u32;
        a += lane_a[i];
    }
    for &byte in tail {
        a += u32::from(byte);
        b += a;
    }
    (a % MOD, b % MOD)
}

/// Computes the Adler-32 of `data` in one call (the zlib checksum, RFC 1950
/// section 8).
///
/// # Examples
///
/// ```
/// use compress_lib::checksum::adler32;
///
/// assert_eq!(adler32(b"Wikipedia"), 0x11E6_0398);
/// assert_eq!(adler32(b""), 1);
/// ```
#[must_use]
pub fn adler32(data: &[u8]) -> u32 {
    let (a, b) = update(1, 0, data);
    (b << 16) | a
}

/// An Adler-32 computed over data that arrives in pieces.
///
/// # Examples
///
/// ```
/// use compress_lib::checksum::{Adler32, adler32};
///
/// let mut sum = Adler32::new();
/// sum.update(b"Wiki");
/// sum.update(b"pedia");
/// assert_eq!(sum.finish(), adler32(b"Wikipedia"));
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Adler32 {
    a: u32,
    b: u32,
    len: u64,
}

impl Adler32 {
    /// An Adler-32 over no data yet (value 1).
    ///
    /// # Examples
    ///
    /// ```
    /// assert_eq!(compress_lib::checksum::Adler32::new().finish(), 1);
    /// ```
    #[must_use]
    pub const fn new() -> Self {
        Self { a: 1, b: 0, len: 0 }
    }

    /// Continues from an Adler-32 value already computed over `len` bytes.
    /// Out-of-range halves (at least 65,521) are reduced first.
    ///
    /// # Examples
    ///
    /// ```
    /// use compress_lib::checksum::{Adler32, adler32};
    ///
    /// let mut sum = Adler32::from_value(adler32(b"Wiki"), 4);
    /// sum.update(b"pedia");
    /// assert_eq!(sum.finish(), adler32(b"Wikipedia"));
    /// ```
    #[must_use]
    pub const fn from_value(adler: u32, len: u64) -> Self {
        Self {
            a: (adler & 0xFFFF) % MOD,
            b: (adler >> 16) % MOD,
            len,
        }
    }

    /// Feeds more data.
    ///
    /// # Examples
    ///
    /// ```
    /// let mut sum = compress_lib::checksum::Adler32::new();
    /// sum.update(b"abc");
    /// assert_eq!(sum.finish(), 0x024D_0127);
    /// ```
    pub fn update(&mut self, data: &[u8]) {
        let (a, b) = update(self.a, self.b, data);
        self.a = a;
        self.b = b;
        self.len = self.len.wrapping_add(data.len() as u64);
    }

    /// The Adler-32 of everything fed so far. Does not reset.
    ///
    /// # Examples
    ///
    /// ```
    /// let mut sum = compress_lib::checksum::Adler32::new();
    /// sum.update(b"a");
    /// assert_eq!(sum.finish(), 0x0062_0062);
    /// ```
    #[must_use]
    pub const fn finish(&self) -> u32 {
        (self.b << 16) | self.a
    }

    /// The number of bytes fed so far (wrapping at 2^64).
    ///
    /// # Examples
    ///
    /// ```
    /// let mut sum = compress_lib::checksum::Adler32::new();
    /// sum.update(b"abc");
    /// assert_eq!(sum.len(), 3);
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
    /// assert!(compress_lib::checksum::Adler32::new().is_empty());
    /// ```
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Appends the checksum of a second piece of data computed separately.
    ///
    /// # Examples
    ///
    /// ```
    /// use compress_lib::checksum::{Adler32, adler32};
    ///
    /// let mut left = Adler32::new();
    /// left.update(b"hello ");
    /// let mut right = Adler32::new();
    /// right.update(b"world");
    /// left.combine(&right);
    /// assert_eq!(left.finish(), adler32(b"hello world"));
    /// ```
    pub fn combine(&mut self, other: &Adler32) {
        *self = Self::from_value(adler32_combine(self.finish(), other.finish(), other.len), 0);
        self.len = self.len.wrapping_add(other.len);
    }
}

impl Default for Adler32 {
    fn default() -> Self {
        Self::new()
    }
}

/// Combines `adler1` (over data A) and `adler2` (over data B, `len2` bytes)
/// into the Adler-32 of A followed by B, in constant time.
///
/// # Examples
///
/// ```
/// use compress_lib::checksum::{adler32, adler32_combine};
///
/// let ab = adler32_combine(adler32(b"hello "), adler32(b"world"), 5);
/// assert_eq!(ab, adler32(b"hello world"));
/// ```
#[must_use]
pub const fn adler32_combine(adler1: u32, adler2: u32, len2: u64) -> u32 {
    let rem = (len2 % MOD as u64) as u32;
    let a1 = (adler1 & 0xFFFF) % MOD;
    let b1 = (adler1 >> 16) % MOD;
    let a2 = (adler2 & 0xFFFF) % MOD;
    let b2 = (adler2 >> 16) % MOD;
    // a = a1 + a2 - 1; b = b1 + b2 + rem * (a1 - 1)  (mod MOD)
    let a = (a1 + a2 + MOD - 1) % MOD;
    let b = (b1 + b2 + (rem * a1) % MOD + MOD - rem) % MOD;
    (b << 16) | a
}

#[cfg(test)]
mod tests {
    use alloc::vec;

    use proptest::prelude::*;

    use super::*;

    fn reference(data: &[u8]) -> u32 {
        let (mut a, mut b) = (1u32, 0u32);
        for &x in data {
            a = (a + u32::from(x)) % MOD;
            b = (b + a) % MOD;
        }
        (b << 16) | a
    }

    #[test]
    fn test_adler32_known_strings_match() {
        assert_eq!(adler32(b""), 1);
        assert_eq!(adler32(b"a"), 0x0062_0062);
        assert_eq!(adler32(b"abc"), 0x024D_0127);
        assert_eq!(adler32(b"Wikipedia"), 0x11E6_0398);
    }

    #[test]
    fn test_adler32_all_ff_past_nmax_does_not_overflow() {
        // 0xFF bytes are the worst case for the deferred modulo.
        for n in [
            NMAX - 1,
            NMAX,
            NMAX + 1,
            NMAX * LANES,
            NMAX * LANES + 3,
            100_000,
        ] {
            let data = vec![0xFFu8; n];
            assert_eq!(adler32(&data), reference(&data), "length {n}");
        }
    }

    #[test]
    fn test_adler32_from_value_reduces_out_of_range() {
        // 0xFFFF in either half is not a valid sum; it reduces modulo 65,521.
        let sum = Adler32::from_value(0xFFFF_FFFF, 0);
        assert_eq!(sum.finish(), (14 << 16) | 14);
        assert_eq!(Adler32::default(), Adler32::new());
    }

    #[test]
    fn test_adler32_combine_empty_second_is_identity() {
        let x = adler32(b"data");
        assert_eq!(adler32_combine(x, 1, 0), x);
    }

    proptest! {
        #[test]
        fn prop_adler32_matches_reference(data in proptest::collection::vec(any::<u8>(), 0..30_000)) {
            prop_assert_eq!(adler32(&data), reference(&data));
        }

        #[test]
        fn prop_adler32_split_invariant(data in proptest::collection::vec(any::<u8>(), 0..4096), cut in any::<usize>()) {
            let cut = if data.is_empty() { 0 } else { cut % (data.len() + 1) };
            let mut sum = Adler32::new();
            sum.update(&data[..cut]);
            sum.update(&data[cut..]);
            prop_assert_eq!(sum.finish(), adler32(&data));
            prop_assert_eq!(sum.len(), data.len() as u64);
        }

        #[test]
        fn prop_adler32_combine_matches_concatenation(a in proptest::collection::vec(any::<u8>(), 0..512), b in proptest::collection::vec(any::<u8>(), 0..70_000)) {
            let mut whole = a.clone();
            whole.extend_from_slice(&b);
            prop_assert_eq!(adler32_combine(adler32(&a), adler32(&b), b.len() as u64), adler32(&whole));
        }
    }
}
