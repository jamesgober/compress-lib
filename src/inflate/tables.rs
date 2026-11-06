//! Huffman decode tables.
//!
//! A table maps the next `root` bits of input (least significant bit first,
//! as DEFLATE packs codes) to an entry; codes longer than `root` go through
//! a subtable. Each entry is a `u32`:
//!
//! | Bits | Meaning |
//! |---|---|
//! | 0 to 7 | bits to consume: code length plus extra bits (link: `root`) |
//! | 8 to 11 | code length within this level (link: subtable index bits) |
//! | 12 | literal |
//! | 13 | end of block |
//! | 14 | subtable link |
//! | 15 | invalid (no code, or a reserved symbol) |
//! | 16 to 31 | value: literal byte, length or distance base, symbol, or subtable offset |

use crate::ErrorKind;

/// Primary index bits of the literal/length table.
pub(crate) const LIT_ROOT: u32 = 11;
/// Primary index bits of the distance table.
pub(crate) const DIST_ROOT: u32 = 8;
/// Index bits of the code-length (precode) table; precode lengths are at
/// most 7, so it never needs a subtable.
pub(crate) const PRE_ROOT: u32 = 7;

/// Largest table any valid literal/length code needs with an 11-bit root
/// (286 symbols, lengths up to 15).
pub(crate) const LIT_ENOUGH: usize = 2342;
/// Largest table any valid distance code needs with an 8-bit root (30
/// symbols, lengths up to 15).
pub(crate) const DIST_ENOUGH: usize = 402;
/// Size of the precode table.
pub(crate) const PRE_ENOUGH: usize = 1 << PRE_ROOT;

pub(crate) const LITERAL: u32 = 1 << 12;
pub(crate) const END_OF_BLOCK: u32 = 1 << 13;
pub(crate) const SUBTABLE: u32 = 1 << 14;
pub(crate) const INVALID: u32 = 1 << 15;

/// The entry for a bit pattern that is not a code.
pub(crate) const INVALID_ENTRY: u32 = INVALID;

const LENGTH_BASE: [u16; 29] = [
    3, 4, 5, 6, 7, 8, 9, 10, 11, 13, 15, 17, 19, 23, 27, 31, 35, 43, 51, 59, 67, 83, 99, 115, 131,
    163, 195, 227, 258,
];
const LENGTH_EXTRA: [u8; 29] = [
    0, 0, 0, 0, 0, 0, 0, 0, 1, 1, 1, 1, 2, 2, 2, 2, 3, 3, 3, 3, 4, 4, 4, 4, 5, 5, 5, 5, 0,
];
const DIST_BASE: [u16; 30] = [
    1, 2, 3, 4, 5, 7, 9, 13, 17, 25, 33, 49, 65, 97, 129, 193, 257, 385, 513, 769, 1025, 1537,
    2049, 3073, 4097, 6145, 8193, 12289, 16385, 24577,
];
const DIST_EXTRA: [u8; 30] = [
    0, 0, 0, 0, 1, 1, 2, 2, 3, 3, 4, 4, 5, 5, 6, 6, 7, 7, 8, 8, 9, 9, 10, 10, 11, 11, 12, 12, 13,
    13,
];

/// Which alphabet a table decodes.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Alphabet {
    /// Literals 0 to 255, end of block 256, lengths 257 to 285.
    LitLen,
    /// Distances 0 to 29.
    Dist,
    /// Code lengths 0 to 18 (extra bits handled by the caller).
    Precode,
}

/// The entry for `symbol` with a code of `len` bits at this table level.
const fn entry(alphabet: Alphabet, symbol: usize, len: u32) -> u32 {
    match alphabet {
        Alphabet::LitLen => {
            if symbol < 256 {
                ((symbol as u32) << 16) | LITERAL | (len << 8) | len
            } else if symbol == 256 {
                END_OF_BLOCK | (len << 8) | len
            } else if symbol < 286 {
                let i = symbol - 257;
                let extra = LENGTH_EXTRA[i] as u32;
                ((LENGTH_BASE[i] as u32) << 16) | (len << 8) | (len + extra)
            } else {
                INVALID | (len << 8) | len
            }
        }
        Alphabet::Dist => {
            if symbol < 30 {
                let extra = DIST_EXTRA[symbol] as u32;
                ((DIST_BASE[symbol] as u32) << 16) | (len << 8) | (len + extra)
            } else {
                INVALID | (len << 8) | len
            }
        }
        Alphabet::Precode => ((symbol as u32) << 16) | (len << 8) | len,
    }
}

/// Reverses the low `len` bits of `code`.
const fn reverse(code: u32, len: u32) -> u32 {
    let mut out = 0;
    let mut i = 0;
    while i < len {
        out |= ((code >> i) & 1) << (len - 1 - i);
        i += 1;
    }
    out
}

/// Builds the decode table for the code lengths `lens` into `table`.
///
/// Follows zlib's acceptance rules: an over-subscribed code is invalid; an
/// incomplete code is invalid except when it is a single code of length 1
/// and `allow_single` is set (literal/length and distance codes); a code
/// with no lengths at all yields a table of invalid entries (the caller
/// decides whether that is acceptable). Entries not covered by an allowed
/// incomplete code are invalid.
pub(crate) const fn build(
    lens: &[u8],
    table: &mut [u32],
    root: u32,
    alphabet: Alphabet,
    allow_single: bool,
) -> Result<(), ErrorKind> {
    let mut count = [0u16; 16];
    let mut i = 0;
    while i < lens.len() {
        count[lens[i] as usize] += 1;
        i += 1;
    }
    count[0] = 0;

    // Kraft sum: over-subscribed when it goes negative.
    let mut left: i32 = 1;
    let mut max = 0u32;
    let mut len = 1;
    while len <= 15 {
        left <<= 1;
        left -= count[len] as i32;
        if left < 0 {
            return Err(ErrorKind::InvalidCodeLengths);
        }
        if count[len] != 0 {
            max = len as u32;
        }
        len += 1;
    }

    let primary = 1usize << root;
    if table.len() < primary {
        return Err(ErrorKind::InvalidCodeLengths);
    }
    let mut j = 0;
    while j < primary {
        table[j] = INVALID_ENTRY;
        j += 1;
    }
    if max == 0 {
        return Ok(());
    }
    if left > 0 && !(allow_single && max == 1) {
        return Err(ErrorKind::InvalidCodeLengths);
    }

    // Symbols sorted by code length, then value: canonical code order.
    let mut offsets = [0u16; 16];
    let mut len = 1;
    while len < 15 {
        offsets[len + 1] = offsets[len] + count[len];
        len += 1;
    }
    let mut sorted = [0u16; 288];
    let mut used = 0usize;
    let mut s = 0;
    while s < lens.len() {
        let l = lens[s] as usize;
        if l != 0 {
            sorted[offsets[l] as usize] = s as u16;
            offsets[l] += 1;
            used += 1;
        }
        s += 1;
    }

    let mut remaining = count;
    let mut code: u32 = 0;
    let mut code_len: u32 = 0;
    let mut next_sub = primary;
    let mut sub_prefix = u32::MAX;
    let mut sub_base = 0usize;
    let mut sub_bits = 0u32;
    let mut k = 0;
    while k < used {
        let symbol = sorted[k] as usize;
        let len = lens[symbol] as u32;
        if code_len != 0 && len > code_len {
            code <<= len - code_len;
        }
        code_len = len;

        if len <= root {
            let e = entry(alphabet, symbol, len);
            let mut j = reverse(code, len) as usize;
            while j < primary {
                table[j] = e;
                j += 1 << len;
            }
        } else {
            let prefix = code >> (len - root);
            if prefix != sub_prefix {
                // Size the subtable as zlib does: grow until the codes still
                // to place fill it.
                let mut bits = len - root;
                let mut room: i32 = 1 << bits;
                while bits + root < max {
                    room -= remaining[(bits + root) as usize] as i32;
                    if room <= 0 {
                        break;
                    }
                    bits += 1;
                    room <<= 1;
                }
                let size = 1usize << bits;
                if next_sub + size > table.len() || next_sub > 0xFFFF {
                    return Err(ErrorKind::InvalidCodeLengths);
                }
                let mut j = 0;
                while j < size {
                    table[next_sub + j] = INVALID_ENTRY;
                    j += 1;
                }
                table[reverse(prefix, root) as usize] =
                    ((next_sub as u32) << 16) | SUBTABLE | (bits << 8) | root;
                sub_prefix = prefix;
                sub_base = next_sub;
                sub_bits = bits;
                next_sub += size;
            }
            let sub_len = len - root;
            let low = code & ((1 << sub_len) - 1);
            let e = entry(alphabet, symbol, sub_len);
            let mut j = reverse(low, sub_len) as usize;
            while j < (1usize << sub_bits) {
                table[sub_base + j] = e;
                j += 1 << sub_len;
            }
        }
        remaining[len as usize] -= 1;
        code += 1;
        k += 1;
    }
    Ok(())
}

/// The fixed literal/length code lengths (RFC 1951 section 3.2.6).
const fn fixed_lit_lens() -> [u8; 288] {
    let mut lens = [0u8; 288];
    let mut i = 0;
    while i < 288 {
        lens[i] = if i < 144 {
            8
        } else if i < 256 {
            9
        } else if i < 280 {
            7
        } else {
            8
        };
        i += 1;
    }
    lens
}

/// The fixed-code literal/length table, built at compile time.
pub(crate) static FIXED_LIT: [u32; 1 << LIT_ROOT] = {
    let mut table = [0u32; 1 << LIT_ROOT];
    let lens = fixed_lit_lens();
    // The fixed lengths form a complete code; `build` cannot fail on them
    // (checked by a test against a run-time build).
    let _ = build(&lens, &mut table, LIT_ROOT, Alphabet::LitLen, true);
    table
};

/// The fixed-code distance table (32 codes of 5 bits; 30 and 31 invalid).
pub(crate) static FIXED_DIST: [u32; 1 << DIST_ROOT] = {
    let mut table = [0u32; 1 << DIST_ROOT];
    let lens = [5u8; 32];
    let _ = build(&lens, &mut table, DIST_ROOT, Alphabet::Dist, true);
    table
};

/// Dynamic tables, one set per inflater (about 11 KiB).
pub(crate) struct Tables {
    pub(crate) lit: [u32; LIT_ENOUGH],
    pub(crate) dist: [u32; DIST_ENOUGH],
    pub(crate) pre: [u32; PRE_ENOUGH],
    /// Code lengths being read: 286 literal/length plus 30 distance, or the
    /// 19 precode lengths.
    pub(crate) lens: [u8; 320],
}

impl Tables {
    pub(crate) fn new() -> Self {
        Self {
            lit: [INVALID_ENTRY; LIT_ENOUGH],
            dist: [INVALID_ENTRY; DIST_ENOUGH],
            pre: [INVALID_ENTRY; PRE_ENOUGH],
            lens: [0; 320],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tables_fixed_match_runtime_build() {
        let mut lit = [0u32; 1 << LIT_ROOT];
        assert_eq!(
            build(
                &fixed_lit_lens(),
                &mut lit,
                LIT_ROOT,
                Alphabet::LitLen,
                true
            ),
            Ok(())
        );
        assert_eq!(lit, FIXED_LIT);
        let mut dist = [0u32; 1 << DIST_ROOT];
        assert_eq!(
            build(&[5u8; 32], &mut dist, DIST_ROOT, Alphabet::Dist, true),
            Ok(())
        );
        assert_eq!(dist, FIXED_DIST);
    }

    #[test]
    fn test_tables_fixed_lit_entries_decode_known_codes() {
        // Literal 0 has fixed code 00110000 (8 bits); reversed, 00001100.
        let e = FIXED_LIT[0b0000_1100];
        assert_ne!(e & LITERAL, 0);
        assert_eq!(e >> 16, 0);
        assert_eq!(e & 0xFF, 8);
        // End of block is 0000000 (7 bits).
        let e = FIXED_LIT[0];
        assert_ne!(e & END_OF_BLOCK, 0);
        assert_eq!(e & 0xFF, 7);
    }

    #[test]
    fn test_tables_oversubscribed_rejected() {
        let mut t = [0u32; 1 << PRE_ROOT];
        assert_eq!(
            build(&[1, 1, 1], &mut t, PRE_ROOT, Alphabet::Precode, false),
            Err(ErrorKind::InvalidCodeLengths)
        );
    }

    #[test]
    fn test_tables_incomplete_rules_follow_zlib() {
        let mut t = [0u32; DIST_ENOUGH];
        // A single code of length 1: allowed for distances, not for the precode.
        assert_eq!(build(&[1], &mut t, DIST_ROOT, Alphabet::Dist, true), Ok(()));
        assert_eq!(t[1], INVALID_ENTRY);
        assert_eq!(
            build(&[1], &mut t, PRE_ROOT, Alphabet::Precode, false),
            Err(ErrorKind::InvalidCodeLengths)
        );
        // A single code of length 2 is incomplete and never allowed.
        assert_eq!(
            build(&[2], &mut t, DIST_ROOT, Alphabet::Dist, true),
            Err(ErrorKind::InvalidCodeLengths)
        );
        // No codes at all: a table of invalid entries.
        assert_eq!(
            build(&[0, 0], &mut t, DIST_ROOT, Alphabet::Dist, true),
            Ok(())
        );
        assert!(t[..1 << DIST_ROOT].iter().all(|&e| e == INVALID_ENTRY));
    }

    #[test]
    fn test_tables_long_codes_use_subtables_and_fit() {
        // A complete code with lengths up to 15: 1, 2, ..., 14, 15, 15.
        let mut lens = [0u8; 30];
        for (i, l) in lens.iter_mut().enumerate().take(15) {
            *l = (i + 1) as u8;
        }
        lens[15] = 15;
        let mut t = [0u32; DIST_ENOUGH];
        assert_eq!(
            build(&lens, &mut t, DIST_ROOT, Alphabet::Dist, true),
            Ok(())
        );
        // The all-ones prefix of 8 bits links to a subtable.
        assert_ne!(t[0xFF] & SUBTABLE, 0);
    }
}
