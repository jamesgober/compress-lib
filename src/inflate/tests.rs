//! Unit tests for the inflater: hand-built streams for every block type,
//! every error, and the edge cases of `dev/PLAN.md` section 10.

use alloc::{vec, vec::Vec};

use proptest::prelude::*;

use super::*;
use crate::deflate::{decompress, decompress_with};

/// Writes bits least significant first, as DEFLATE packs them.
struct Bits {
    out: Vec<u8>,
    acc: u64,
    n: u32,
}

impl Bits {
    fn new() -> Self {
        Self {
            out: Vec::new(),
            acc: 0,
            n: 0,
        }
    }

    /// Writes the low `count` bits of `value`.
    fn bits(&mut self, value: u32, count: u32) -> &mut Self {
        self.acc |= u64::from(value) << self.n;
        self.n += count;
        while self.n >= 8 {
            self.out.push(self.acc as u8);
            self.acc >>= 8;
            self.n -= 8;
        }
        self
    }

    /// Writes a Huffman code (most significant bit first).
    fn code(&mut self, code: u32, len: u32) -> &mut Self {
        let mut r = 0;
        for i in 0..len {
            r |= ((code >> i) & 1) << (len - 1 - i);
        }
        self.bits(r, len)
    }

    /// Pads to a byte boundary.
    fn align(&mut self) -> &mut Self {
        if self.n > 0 {
            let pad = 8 - self.n;
            self.bits(0, pad);
        }
        self
    }

    fn bytes(&mut self, data: &[u8]) -> &mut Self {
        for &b in data {
            self.bits(u32::from(b), 8);
        }
        self
    }

    fn done(&mut self) -> Vec<u8> {
        self.align();
        core::mem::take(&mut self.out)
    }

    /// A fixed-code literal/length symbol.
    fn fixed_symbol(&mut self, sym: u32) -> &mut Self {
        match sym {
            0..=143 => self.code(0x30 + sym, 8),
            144..=255 => self.code(0x190 + sym - 144, 9),
            256..=279 => self.code(sym - 256, 7),
            _ => self.code(0xC0 + sym - 280, 8),
        }
    }

    fn fixed_literal(&mut self, b: u8) -> &mut Self {
        self.fixed_symbol(u32::from(b))
    }

    /// A fixed-code match of `len` (3 to 258) at `dist` (1 to 32768).
    fn fixed_match(&mut self, len: u32, dist: u32) -> &mut Self {
        let (sym, extra, base) = length_code(len);
        self.fixed_symbol(sym);
        self.bits(len - base, extra);
        let (code, extra, base) = dist_code(dist);
        self.code(code, 5);
        self.bits(dist - base, extra)
    }
}

fn length_code(len: u32) -> (u32, u32, u32) {
    const BASE: [u32; 29] = [
        3, 4, 5, 6, 7, 8, 9, 10, 11, 13, 15, 17, 19, 23, 27, 31, 35, 43, 51, 59, 67, 83, 99, 115,
        131, 163, 195, 227, 258,
    ];
    const EXTRA: [u32; 29] = [
        0, 0, 0, 0, 0, 0, 0, 0, 1, 1, 1, 1, 2, 2, 2, 2, 3, 3, 3, 3, 4, 4, 4, 4, 5, 5, 5, 5, 0,
    ];
    let i = (0..29).rev().find(|&i| BASE[i] <= len).unwrap();
    (257 + i as u32, EXTRA[i], BASE[i])
}

fn dist_code(dist: u32) -> (u32, u32, u32) {
    const BASE: [u32; 30] = [
        1, 2, 3, 4, 5, 7, 9, 13, 17, 25, 33, 49, 65, 97, 129, 193, 257, 385, 513, 769, 1025, 1537,
        2049, 3073, 4097, 6145, 8193, 12289, 16385, 24577,
    ];
    let i = (0..30).rev().find(|&i| BASE[i] <= dist).unwrap();
    (
        i as u32,
        if i < 4 { 0 } else { (i as u32 - 2) / 2 },
        BASE[i],
    )
}

/// A final fixed block holding `body` (written by the closure), then the end
/// of block.
fn fixed_block(body: impl FnOnce(&mut Bits)) -> Vec<u8> {
    let mut w = Bits::new();
    w.bits(1, 1).bits(1, 2);
    body(&mut w);
    w.fixed_symbol(256);
    w.done()
}

/// A stored block (final or not) with `data`.
fn stored(w: &mut Bits, data: &[u8], last: bool) {
    w.bits(u32::from(last), 1).bits(0, 2).align();
    let len = data.len() as u32;
    w.bits(len, 16).bits(!len & 0xFFFF, 16).bytes(data);
}

fn kind(r: Result<Vec<u8>, Error>) -> ErrorKind {
    r.unwrap_err().kind()
}

/// Decodes with every input split and output size given, collecting output.
fn decode_chunked(data: &[u8], in_step: usize, out_step: usize) -> Result<Vec<u8>, Error> {
    let mut inflater = Inflater::new();
    let mut out = Vec::new();
    let mut buf = vec![0u8; out_step];
    let mut pos = 0;
    loop {
        let end = (pos + in_step).min(data.len());
        let last = end == data.len();
        let p = if last {
            inflater.finish(&data[pos..end], &mut buf)?
        } else {
            inflater.decompress(&data[pos..end], &mut buf)?
        };
        assert!(p.consumed <= end - pos);
        assert!(p.produced <= buf.len());
        out.extend_from_slice(&buf[..p.produced]);
        pos += p.consumed;
        match p.status {
            Status::Done => return Ok(out),
            Status::NeedsInput => assert_eq!(pos, end, "NeedsInput must consume all input"),
            Status::NeedsOutput => {}
        }
    }
}

#[test]
fn test_inflate_fixed_hello_matches_zlib_output() {
    assert_eq!(
        decompress(&[0xCB, 0x48, 0xCD, 0xC9, 0xC9, 0x07, 0x00]).unwrap(),
        b"hello"
    );
}

#[test]
fn test_inflate_empty_input_is_unexpected_end() {
    assert_eq!(kind(decompress(&[])), ErrorKind::UnexpectedEnd);
}

#[test]
fn test_inflate_one_byte_is_unexpected_end() {
    assert_eq!(kind(decompress(&[0x03])), ErrorKind::UnexpectedEnd);
}

#[test]
fn test_inflate_empty_fixed_block_decodes_to_nothing() {
    assert_eq!(decompress(&fixed_block(|_| {})).unwrap(), b"");
    assert_eq!(decompress(&[0x03, 0x00]).unwrap(), b"");
}

#[test]
fn test_inflate_block_type_3_rejected() {
    let mut w = Bits::new();
    w.bits(1, 1).bits(3, 2);
    assert_eq!(kind(decompress(&w.done())), ErrorKind::InvalidBlockType);
}

#[test]
fn test_inflate_stored_round_trip_and_sizes() {
    for len in [0usize, 1, 2, 255, 65_535] {
        let data: Vec<u8> = (0..len).map(|i| (i * 7) as u8).collect();
        let mut w = Bits::new();
        stored(&mut w, &data, true);
        assert_eq!(decompress(&w.done()).unwrap(), data, "length {len}");
    }
    // 65,536 bytes need two stored blocks.
    let data = vec![0xA5u8; 65_536];
    let mut w = Bits::new();
    stored(&mut w, &data[..65_535], false);
    stored(&mut w, &data[65_535..], true);
    assert_eq!(decompress(&w.done()).unwrap(), data);
}

#[test]
fn test_inflate_stored_length_mismatch_rejected() {
    let mut w = Bits::new();
    w.bits(1, 1).bits(0, 2).align().bits(5, 16).bits(5, 16);
    assert_eq!(kind(decompress(&w.done())), ErrorKind::InvalidStoredLength);
}

#[test]
fn test_inflate_many_empty_stored_blocks_linear() {
    let mut w = Bits::new();
    for _ in 0..10_000 {
        stored(&mut w, &[], false);
    }
    stored(&mut w, b"x", true);
    assert_eq!(decompress(&w.done()).unwrap(), b"x");
}

#[test]
fn test_inflate_match_lengths_3_and_258_and_distance_1() {
    let data = fixed_block(|w| {
        w.fixed_literal(b'a').fixed_match(3, 1).fixed_match(258, 1);
    });
    assert_eq!(decompress(&data).unwrap(), vec![b'a'; 262]);
}

#[test]
fn test_inflate_every_length_and_short_distance_matches_reference() {
    for dist in 1..=12u32 {
        for len in [3u32, 4, 7, 8, 9, 15, 16, 17, 31, 64, 100, 257, 258] {
            let prefix: Vec<u8> = (0..dist).map(|i| b'a' + i as u8).collect();
            let data = fixed_block(|w| {
                for &b in &prefix {
                    w.fixed_literal(b);
                }
                w.fixed_match(len, dist);
            });
            let mut want = prefix.clone();
            for _ in 0..len {
                let b = want[want.len() - dist as usize];
                want.push(b);
            }
            assert_eq!(decompress(&data).unwrap(), want, "len {len} dist {dist}");
        }
    }
}

#[test]
fn test_inflate_distance_32768_ok_and_one_past_history_rejected() {
    let lits = vec![b'z'; 32_768];
    let mut w = Bits::new();
    stored(&mut w, &lits, false);
    w.bits(1, 1)
        .bits(1, 2)
        .fixed_match(3, 32_768)
        .fixed_symbol(256);
    let out = decompress(&w.done()).unwrap();
    assert_eq!(out.len(), 32_771);

    let mut w = Bits::new();
    stored(&mut w, &lits[..32_767], false);
    w.bits(1, 1)
        .bits(1, 2)
        .fixed_match(3, 32_768)
        .fixed_symbol(256);
    assert_eq!(kind(decompress(&w.done())), ErrorKind::DistanceTooFar);
}

#[test]
fn test_inflate_distance_before_start_rejected() {
    let data = fixed_block(|w| {
        w.fixed_literal(b'a').fixed_match(3, 2);
    });
    assert_eq!(kind(decompress(&data)), ErrorKind::DistanceTooFar);
}

#[test]
fn test_inflate_window_bits_8_enforces_256() {
    let lits = vec![b'q'; 300];
    let build = |dist| {
        let mut w = Bits::new();
        stored(&mut w, &lits, false);
        w.bits(1, 1)
            .bits(1, 2)
            .fixed_match(3, dist)
            .fixed_symbol(256);
        w.done()
    };
    let mut out = Vec::new();
    let mut ok = Inflater::new().with_window_bits(8).unwrap();
    assert_eq!(
        ok.finish_vec(&build(256), &mut out).unwrap().status,
        Status::Done
    );
    let mut bad = Inflater::new().with_window_bits(8).unwrap();
    assert_eq!(
        bad.finish_vec(&build(257), &mut Vec::new())
            .unwrap_err()
            .kind(),
        ErrorKind::DistanceTooFar
    );
    assert_eq!(
        Inflater::new().with_window_bits(7).unwrap_err().kind(),
        ErrorKind::InvalidWindowBits
    );
}

#[test]
fn test_inflate_reserved_fixed_symbols_rejected() {
    for sym in [286, 287] {
        let mut w = Bits::new();
        w.bits(1, 1)
            .bits(1, 2)
            .fixed_literal(b'a')
            .fixed_symbol(sym);
        assert_eq!(
            kind(decompress(&w.done())),
            ErrorKind::InvalidSymbol,
            "symbol {sym}"
        );
    }
    for code in [30, 31] {
        let mut w = Bits::new();
        w.bits(1, 1)
            .bits(1, 2)
            .fixed_literal(b'a')
            .fixed_symbol(257)
            .code(code, 5);
        assert_eq!(
            kind(decompress(&w.done())),
            ErrorKind::InvalidSymbol,
            "distance {code}"
        );
    }
}

/// Writes a dynamic block header with the given literal/length and distance
/// code lengths. The precode is complete: symbols 0 to 12 get 4 bits and
/// 13 to 18 get 5 bits, so symbol 16 is code 29 and 18 is code 31.
fn dynamic_header(w: &mut Bits, last: bool, lit: &[u8], dist: &[u8]) -> [(u32, u32); 19] {
    w.bits(u32::from(last), 1).bits(2, 2);
    w.bits(lit.len() as u32 - 257, 5)
        .bits(dist.len() as u32 - 1, 5)
        .bits(19 - 4, 4);
    let mut pre_len = [0u32; 19];
    for (s, l) in pre_len.iter_mut().enumerate() {
        *l = if s <= 12 { 4 } else { 5 };
    }
    for &s in &PRECODE_ORDER {
        w.bits(pre_len[s as usize], 3);
    }
    // Canonical codes for the precode.
    let mut codes = [(0u32, 0u32); 19];
    let mut code = 0u32;
    for len in 4..=5 {
        for s in 0..19 {
            if pre_len[s] == len {
                codes[s] = (code, len);
                code += 1;
            }
        }
        code <<= 1;
    }
    for &l in lit.iter().chain(dist) {
        let (c, n) = codes[l as usize];
        w.code(c, n);
    }
    codes
}

/// Canonical codes for lengths.
fn canonical(lens: &[u8]) -> Vec<(u32, u32)> {
    let mut out = vec![(0, 0); lens.len()];
    let mut code = 0u32;
    for len in 1..=15u8 {
        for (s, &l) in lens.iter().enumerate() {
            if l == len {
                out[s] = (code, u32::from(len));
                code += 1;
            }
        }
        code <<= 1;
    }
    out
}

#[test]
fn test_inflate_dynamic_block_literals_only_without_distance_codes() {
    // Literals 'a' and 'b' and end of block, 2 bits each except one; no
    // distance codes at all (one distance length of 0).
    let mut lit = vec![0u8; 257];
    lit[usize::from(b'a')] = 1;
    lit[usize::from(b'b')] = 2;
    lit[256] = 2;
    let mut w = Bits::new();
    dynamic_header(&mut w, true, &lit, &[0]);
    let c = canonical(&lit);
    for &b in b"abba" {
        let (code, n) = c[usize::from(b)];
        w.code(code, n);
    }
    w.code(c[256].0, c[256].1);
    assert_eq!(decompress(&w.done()).unwrap(), b"abba");
}

#[test]
fn test_inflate_dynamic_single_distance_code_allowed() {
    let mut lit = vec![0u8; 258];
    lit[usize::from(b'a')] = 1;
    lit[256] = 2;
    lit[257] = 2; // length 3
    let mut w = Bits::new();
    dynamic_header(&mut w, true, &lit, &[1]);
    let c = canonical(&lit);
    w.code(c[97].0, 1)
        .code(c[257].0, 2)
        .code(0, 1)
        .code(c[256].0, 2);
    assert_eq!(decompress(&w.done()).unwrap(), b"aaaa");
}

#[test]
fn test_inflate_dynamic_missing_end_of_block_rejected() {
    let mut lit = vec![0u8; 257];
    lit[0] = 1;
    lit[1] = 1;
    let mut w = Bits::new();
    dynamic_header(&mut w, true, &lit, &[0]);
    assert_eq!(kind(decompress(&w.done())), ErrorKind::InvalidCodeLengths);
}

#[test]
fn test_inflate_dynamic_too_many_symbols_rejected() {
    let mut w = Bits::new();
    w.bits(1, 1).bits(2, 2).bits(30, 5).bits(0, 5).bits(0, 4);
    assert_eq!(kind(decompress(&w.done())), ErrorKind::InvalidCodeLengths);
    let mut w = Bits::new();
    w.bits(1, 1).bits(2, 2).bits(0, 5).bits(30, 5).bits(0, 4);
    assert_eq!(kind(decompress(&w.done())), ErrorKind::InvalidCodeLengths);
}

#[test]
fn test_inflate_dynamic_repeat_without_previous_rejected() {
    let mut w = Bits::new();
    w.bits(1, 1).bits(2, 2).bits(0, 5).bits(0, 5).bits(15, 4);
    // Precode as in `dynamic_header`.
    for &s in &PRECODE_ORDER {
        w.bits(if s <= 12 { 4 } else { 5 }, 3);
    }
    w.code(29, 5).bits(0, 2);
    assert_eq!(kind(decompress(&w.done())), ErrorKind::InvalidCodeLengths);
}

#[test]
fn test_inflate_dynamic_repeat_past_end_rejected() {
    let mut w = Bits::new();
    w.bits(1, 1).bits(2, 2).bits(0, 5).bits(0, 5).bits(15, 4);
    for &s in &PRECODE_ORDER {
        w.bits(if s <= 12 { 4 } else { 5 }, 3);
    }
    // 257 + 1 lengths: symbol 18 repeats zero 138 times, twice (276), past 258.
    w.code(31, 5).bits(127, 7).code(31, 5).bits(127, 7);
    assert_eq!(kind(decompress(&w.done())), ErrorKind::InvalidCodeLengths);
}

#[test]
fn test_inflate_dynamic_oversubscribed_rejected() {
    let lit = vec![1u8; 257];
    let mut w = Bits::new();
    dynamic_header(&mut w, true, &lit, &[1]);
    assert_eq!(kind(decompress(&w.done())), ErrorKind::InvalidCodeLengths);
}

#[test]
fn test_inflate_dynamic_long_codes_through_subtables() {
    // Literal 0 gets 1 bit, 1 gets 2, ..., up to 15-bit codes, so decoding
    // walks the subtables of both tables.
    let mut lit = vec![0u8; 257];
    for (i, l) in lit.iter_mut().enumerate().take(14) {
        *l = (i + 1) as u8;
    }
    lit[14] = 15;
    lit[256] = 15;
    let mut w = Bits::new();
    dynamic_header(&mut w, true, &lit, &[0]);
    let c = canonical(&lit);
    let text: Vec<u8> = (0..15).chain((0..15).rev()).collect();
    for &b in &text {
        w.code(c[usize::from(b)].0, c[usize::from(b)].1);
    }
    w.code(c[256].0, c[256].1);
    assert_eq!(decompress(&w.done()).unwrap(), text);
}

#[test]
fn test_inflate_truncated_at_every_byte_is_unexpected_end() {
    let data = fixed_block(|w| {
        for &b in b"truncate me, truncate me" {
            w.fixed_literal(b);
        }
        w.fixed_match(20, 13);
    });
    for n in 0..data.len() {
        assert_eq!(
            kind(decompress(&data[..n])),
            ErrorKind::UnexpectedEnd,
            "prefix {n}"
        );
    }
    assert!(decompress(&data).is_ok());
}

#[test]
fn test_inflate_trailing_bytes_rejected_and_consumed_exact() {
    let mut data = fixed_block(|w| {
        w.fixed_literal(b'x');
    });
    let n = data.len();
    data.extend_from_slice(b"TRAILER");
    assert_eq!(kind(decompress(&data)), ErrorKind::TrailingData);
    let mut out = [0u8; 8];
    let p = Inflater::new().decompress(&data, &mut out).unwrap();
    assert_eq!((p.consumed, p.produced, p.status), (n, 1, Status::Done));
}

#[test]
fn test_inflate_output_cap_exact_and_one_past() {
    let data = fixed_block(|w| {
        w.fixed_literal(0).fixed_match(258, 1).fixed_match(258, 1);
    });
    let limits = Limits::new().with_max_output(517);
    assert_eq!(decompress_with(&data, limits).unwrap().len(), 517);
    let limits = Limits::new().with_max_output(516);
    assert_eq!(kind(decompress_with(&data, limits)), ErrorKind::OutputLimit);
    // The streaming API never writes past the cap either.
    let mut inflater = Inflater::new().with_limits(limits);
    let mut out = [0u8; 1024];
    assert_eq!(
        inflater.finish(&data, &mut out).unwrap_err().kind(),
        ErrorKind::OutputLimit
    );
    assert_eq!(inflater.total_out(), 516);
    assert!(out[516..].iter().all(|&b| b == 0));
}

#[test]
fn test_inflate_ratio_cap_fires_after_floor() {
    let data = fixed_block(|w| {
        w.fixed_literal(0);
        for _ in 0..100 {
            w.fixed_match(258, 1);
        }
    });
    let limits = Limits::new().with_max_ratio(100).with_ratio_floor(1000);
    assert_eq!(kind(decompress_with(&data, limits)), ErrorKind::RatioLimit);
    let generous = Limits::new().with_max_ratio(1000).with_ratio_floor(1000);
    assert_eq!(decompress_with(&data, generous).unwrap().len(), 25_801);
}

#[test]
fn test_inflate_ratio_cap_counts_buffered_input_byte_at_a_time() {
    // The ratio error fires at the same output whether the input arrives at
    // once or one byte per call (PLAN.md section 15, idea 0.1).
    let data = fixed_block(|w| {
        w.fixed_literal(0);
        for _ in 0..100 {
            w.fixed_match(258, 1);
        }
    });
    let limits = Limits::new().with_max_ratio(100).with_ratio_floor(1000);
    let mut whole = Inflater::new().with_limits(limits);
    let mut out = vec![0u8; 1 << 16];
    let e1 = whole.finish(&data, &mut out).unwrap_err();
    let mut bytewise = Inflater::new().with_limits(limits);
    let mut produced = 0;
    let mut e2 = None;
    for (i, b) in data.iter().enumerate() {
        let r = if i + 1 == data.len() {
            bytewise.finish(core::slice::from_ref(b), &mut out[produced..])
        } else {
            bytewise.decompress(core::slice::from_ref(b), &mut out[produced..])
        };
        match r {
            Ok(p) => produced += p.produced,
            Err(e) => {
                e2 = Some(e);
                break;
            }
        }
    }
    assert_eq!(e1.kind(), ErrorKind::RatioLimit);
    assert_eq!(e2.map(|e| e.kind()), Some(ErrorKind::RatioLimit));
    // Read-ahead differs by at most eight bytes, so the stop point differs
    // by at most eight bytes' worth of ratio.
    assert!(whole.total_out().abs_diff(bytewise.total_out()) <= 8 * 100);
}

#[test]
fn test_inflate_end_of_block_only_code_allowed() {
    let mut lit = vec![0u8; 257];
    lit[256] = 1;
    let mut w = Bits::new();
    dynamic_header(&mut w, true, &lit, &[0]);
    w.code(0, 1);
    assert_eq!(decompress(&w.done()).unwrap(), b"");
}

#[test]
fn test_inflate_errors_are_sticky_until_reset() {
    let mut inflater = Inflater::new();
    let mut out = [0u8; 4];
    let bad = [0x07]; // final block of type 3
    let e = inflater.decompress(&bad, &mut out).unwrap_err();
    assert_eq!(inflater.decompress(&[0x03, 0x00], &mut out).unwrap_err(), e);
    inflater.reset();
    assert_eq!(
        inflater.finish(&[0x03, 0x00], &mut out).unwrap().status,
        Status::Done
    );
}

#[test]
fn test_inflate_tiny_buffers_and_splits_equal_one_shot() {
    let data = fixed_block(|w| {
        for i in 0..200u32 {
            w.fixed_literal((i * 37) as u8);
        }
        w.fixed_match(258, 200)
            .fixed_match(100, 1)
            .fixed_match(77, 150);
    });
    let want = decompress(&data).unwrap();
    for in_step in [1usize, 2, 3, 7, 64, 1000] {
        for out_step in 1..=40 {
            assert_eq!(
                decode_chunked(&data, in_step, out_step).unwrap(),
                want,
                "{in_step}/{out_step}"
            );
        }
    }
}

#[test]
fn test_inflate_history_across_calls_uses_ring() {
    // Output buffers smaller than the distances force every match to read
    // the ring.
    let mut w = Bits::new();
    let text: Vec<u8> = (0..5000u32).map(|i| (i % 251) as u8).collect();
    stored(&mut w, &text, false);
    w.bits(1, 1).bits(1, 2);
    for _ in 0..20 {
        w.fixed_match(258, 4999);
    }
    w.fixed_symbol(256);
    let data = w.done();
    let want = decompress(&data).unwrap();
    assert_eq!(decode_chunked(&data, 10, 100).unwrap(), want);
    assert_eq!(decode_chunked(&data, 3, 1).unwrap(), want);
}

#[test]
fn test_inflate_memory_in_use_matches_window() {
    let big = Inflater::new();
    let small = Inflater::new().with_window_bits(9).unwrap();
    assert_eq!(
        big.memory_in_use() - small.memory_in_use(),
        (1 << 15) - (1 << 9)
    );
    assert!(core::mem::size_of::<Tables>() < 12 * 1024);
}

proptest! {
    #[test]
    fn prop_inflate_never_panics(data in proptest::collection::vec(any::<u8>(), 0..512)) {
        let _ = decompress(&data);
        let _ = decode_chunked(&data, 3, 5);
    }

    #[test]
    fn prop_inflate_stored_round_trip_any_split(
        data in proptest::collection::vec(any::<u8>(), 0..3000),
        in_step in 1usize..64,
        out_step in 1usize..64,
    ) {
        let mut w = Bits::new();
        stored(&mut w, &data, true);
        let packed = w.done();
        prop_assert_eq!(decode_chunked(&packed, in_step, out_step).unwrap(), data);
    }
}

#[test]
fn test_inflate_next_stream_before_done_is_invalid_state() {
    let mut inflater = Inflater::new();
    assert_eq!(
        inflater.next_stream().unwrap_err().kind(),
        ErrorKind::InvalidState
    );
}

#[test]
fn test_inflate_next_stream_keeps_window() {
    // Stream 1 ends with "abcabc"; stream 2 is one match at distance 6.
    let first = fixed_block(|w| {
        for &b in b"abcabc" {
            w.fixed_literal(b);
        }
    });
    let second = fixed_block(|w| {
        w.fixed_match(6, 6);
    });
    let mut inflater = Inflater::new();
    let mut out = Vec::new();
    inflater.finish_vec(&first, &mut out).unwrap();
    inflater.next_stream().unwrap();
    let mut more = Vec::new();
    inflater.finish_vec(&second, &mut more).unwrap();
    assert_eq!(more, b"abcabc");
    // Without the kept window the same stream is invalid.
    assert_eq!(kind(decompress(&second)), ErrorKind::DistanceTooFar);
}

#[test]
fn test_inflate_set_dictionary_feeds_history_and_only_at_start() {
    let stream = fixed_block(|w| {
        w.fixed_match(4, 4).fixed_literal(b'!');
    });
    let mut inflater = Inflater::new();
    inflater.set_dictionary(b"xxwxyz").unwrap();
    let mut out = Vec::new();
    inflater.finish_vec(&stream, &mut out).unwrap();
    assert_eq!(out, b"wxyz!");
    assert_eq!(
        inflater.set_dictionary(b"late").unwrap_err().kind(),
        ErrorKind::InvalidState
    );
    // A dictionary longer than the window keeps its last bytes.
    let mut small = Inflater::new().with_window_bits(8).unwrap();
    let mut dict = vec![b'-'; 1000];
    dict.extend_from_slice(b"wxyz");
    small.set_dictionary(&dict).unwrap();
    let mut out = Vec::new();
    small.finish_vec(&stream, &mut out).unwrap();
    assert_eq!(out, b"wxyz!");
}

#[test]
fn test_inflate_wipe_zeroes_window_and_tables() {
    let mut inflater = Inflater::new();
    let data = b"secret secret secret secret".repeat(40);
    let packed = {
        let mut w = Bits::new();
        stored(&mut w, &data, true);
        w.done()
    };
    let mut out = vec![0u8; 2048];
    inflater.finish(&packed, &mut out).unwrap();
    assert!(inflater.ring.iter().any(|&b| b != 0));
    inflater.wipe();
    assert!(inflater.ring.iter().all(|&b| b == 0));
    assert!(inflater.tables.lens.iter().all(|&b| b == 0));
    assert_eq!((inflater.whave, inflater.total_out), (0, 0));
    assert_eq!(
        inflater.finish(&[0x03, 0x00], &mut out).unwrap().status,
        Status::Done
    );
}
