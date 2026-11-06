//! Huffman code construction for the encoder: length-limited code lengths
//! from symbol frequencies, and canonical codes from lengths.

/// Largest alphabet the encoder builds a code for (literal/length).
pub(crate) const MAX_SYMBOLS: usize = 288;

/// Computes code lengths for `freq` (zero frequency gets length 0), at most
/// `limit` bits, forming a complete prefix code whenever two or more
/// symbols are used (one used symbol gets length 1). Deterministic: ties
/// break by symbol number.
pub(crate) fn lengths(freq: &[u32], limit: u32, lens: &mut [u8]) {
    let n_all = freq.len();
    lens[..n_all].fill(0);

    // Used symbols, sorted by (frequency, symbol).
    let mut order = [0u16; MAX_SYMBOLS];
    let mut n = 0;
    for (s, &f) in freq.iter().enumerate() {
        if f != 0 {
            order[n] = s as u16;
            n += 1;
        }
    }
    let order = &mut order[..n];
    order.sort_unstable_by_key(|&s| (freq[s as usize], s));
    match n {
        0 => return,
        1 => {
            lens[order[0] as usize] = 1;
            return;
        }
        _ => {}
    }

    // Huffman's algorithm with two queues: sorted leaves, and internal
    // nodes, which are created in non-decreasing weight order.
    let mut weight = [0u64; MAX_SYMBOLS];
    let mut leaf_parent = [0u16; MAX_SYMBOLS];
    let mut node_parent = [0u16; MAX_SYMBOLS];
    let (mut leaf, mut front, mut back) = (0usize, 0usize, 0usize);
    let pick =
        |leaf: &mut usize, front: &mut usize, back: usize, weight: &[u64]| -> (bool, usize) {
            let take_leaf = *leaf < n
                && (*front >= back || u64::from(freq[order[*leaf] as usize]) <= weight[*front]);
            if take_leaf {
                *leaf += 1;
                (true, *leaf - 1)
            } else {
                *front += 1;
                (false, *front - 1)
            }
        };
    for _ in 0..n - 1 {
        let mut w = 0;
        for _ in 0..2 {
            let (is_leaf, i) = pick(&mut leaf, &mut front, back, &weight);
            if is_leaf {
                w += u64::from(freq[order[i] as usize]);
                leaf_parent[i] = back as u16;
            } else {
                w += weight[i];
                node_parent[i] = back as u16;
            }
        }
        weight[back] = w;
        back += 1;
    }
    // Depths: the root is the last node created.
    let mut depth = [0u32; MAX_SYMBOLS];
    for j in (0..back - 1).rev() {
        depth[j] = depth[node_parent[j] as usize] + 1;
    }
    let mut len = [0u32; MAX_SYMBOLS];
    for i in 0..n {
        len[i] = (depth[leaf_parent[i] as usize] + 1).min(limit);
    }

    // Repair the Kraft sum after clamping: lengthen the rarest symbols until
    // the code is not over-full, then shorten the longest until it is
    // complete. `len` is in `order`, so index 0 is the rarest.
    let full = 1u64 << limit;
    let mut kraft: u64 = len[..n].iter().map(|&l| 1u64 << (limit - l)).sum();
    while kraft > full {
        for l in len[..n].iter_mut() {
            if *l < limit {
                kraft -= 1u64 << (limit - *l - 1);
                *l += 1;
                if kraft <= full {
                    break;
                }
            }
        }
    }
    while kraft < full {
        // The longest code, most frequent among ties (the latest in order).
        let mut best = 0;
        for i in 0..n {
            if len[i] >= len[best] {
                best = i;
            }
        }
        kraft += 1u64 << (limit - len[best]);
        len[best] -= 1;
    }
    for i in 0..n {
        lens[order[i] as usize] = len[i] as u8;
    }
}

/// Assigns canonical codes to `lens` and stores each reversed (least
/// significant bit first, as DEFLATE writes them) in `codes`.
pub(crate) fn codes(lens: &[u8], codes: &mut [u16]) {
    let mut count = [0u16; 16];
    for &l in lens {
        count[l as usize] += 1;
    }
    count[0] = 0;
    let mut next = [0u16; 16];
    let mut code = 0u16;
    for bits in 1..16 {
        code = (code + count[bits - 1]) << 1;
        next[bits] = code;
    }
    for (s, &l) in lens.iter().enumerate() {
        if l != 0 {
            let c = next[l as usize];
            next[l as usize] += 1;
            codes[s] = c.reverse_bits() >> (16 - l);
        } else {
            codes[s] = 0;
        }
    }
}

#[cfg(test)]
mod tests {
    use alloc::vec;

    use proptest::prelude::*;

    use super::*;

    fn kraft(lens: &[u8], limit: u32) -> u64 {
        lens.iter()
            .filter(|&&l| l != 0)
            .map(|&l| 1u64 << (limit - u32::from(l)))
            .sum()
    }

    #[test]
    fn test_huffman_lengths_classic_example() {
        let freq = [5u32, 9, 12, 13, 16, 45];
        let mut lens = [0u8; 6];
        lengths(&freq, 15, &mut lens);
        assert_eq!(lens, [4, 4, 3, 3, 3, 1]);
    }

    #[test]
    fn test_huffman_lengths_zero_and_one_symbol() {
        let mut lens = [9u8; 4];
        lengths(&[0, 0, 0, 0], 15, &mut lens);
        assert_eq!(lens, [0; 4]);
        lengths(&[0, 7, 0, 0], 15, &mut lens);
        assert_eq!(lens, [0, 1, 0, 0]);
    }

    #[test]
    fn test_huffman_lengths_fibonacci_clamped_complete() {
        // Fibonacci frequencies give the deepest tree: 30 symbols would need
        // depth 29 unlimited.
        let mut freq = vec![1u32, 1];
        while freq.len() < 30 {
            let k = freq.len();
            freq.push(freq[k - 1] + freq[k - 2]);
        }
        let mut lens = vec![0u8; 30];
        lengths(&freq, 15, &mut lens);
        assert!(lens.iter().all(|&l| (1..=15).contains(&l)));
        assert_eq!(kraft(&lens, 15), 1 << 15);
        let mut lens7 = vec![0u8; 19];
        lengths(&freq[..19], 7, &mut lens7);
        assert!(lens7.iter().all(|&l| (1..=7).contains(&l)));
        assert_eq!(kraft(&lens7, 7), 1 << 7);
    }

    #[test]
    fn test_huffman_codes_canonical_rfc_example() {
        // RFC 1951 section 3.2.2: lengths (3, 3, 3, 3, 3, 2, 4, 4) give codes
        // 010, 011, 100, 101, 110, 00, 1110, 1111.
        let lens = [3u8, 3, 3, 3, 3, 2, 4, 4];
        let mut c = [0u16; 8];
        codes(&lens, &mut c);
        let msb = [0b010u16, 0b011, 0b100, 0b101, 0b110, 0b00, 0b1110, 0b1111];
        for i in 0..8 {
            assert_eq!(c[i], msb[i].reverse_bits() >> (16 - lens[i]), "symbol {i}");
        }
    }

    proptest! {
        #[test]
        fn prop_huffman_lengths_complete_and_limited(
            freq in proptest::collection::vec(0u32..100_000, 2..288),
            limit in 7u32..=15,
        ) {
            let used = freq.iter().filter(|&&f| f != 0).count();
            prop_assume!(used as u64 <= 1 << limit);
            let mut lens = vec![0u8; freq.len()];
            lengths(&freq, limit, &mut lens);
            for (f, l) in freq.iter().zip(&lens) {
                prop_assert_eq!(*f == 0, *l == 0);
                prop_assert!(u32::from(*l) <= limit);
            }
            if used >= 2 {
                prop_assert_eq!(kraft(&lens, limit), 1u64 << limit);
            }
        }
    }
}
