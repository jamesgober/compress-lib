//! One attack test per threat in `dev/PLAN.md` section 8 (T-1 to T-14).
//! Each test is the attack; it passes when the defence holds.

use std::time::{Duration, Instant};

use compress_lib::{
    ErrorKind, Level, Limits,
    deflate::{self, Deflater, Flush, Inflater, Status},
    gzip,
    message::{Compressor, Decompressor, MessageConfig},
    zlib,
};

fn next(x: &mut u32) -> u32 {
    *x ^= *x << 13;
    *x ^= *x >> 17;
    *x ^= *x << 5;
    *x
}

/// Writes bits least significant first.
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
    fn bits(&mut self, v: u32, n: u32) -> &mut Self {
        self.acc |= u64::from(v) << self.n;
        self.n += n;
        while self.n >= 8 {
            self.out.push(self.acc as u8);
            self.acc >>= 8;
            self.n -= 8;
        }
        self
    }
    fn done(&mut self) -> Vec<u8> {
        if self.n > 0 {
            let pad = 8 - self.n;
            self.bits(0, pad);
        }
        std::mem::take(&mut self.out)
    }
}

/// T-1: a decompression bomb stops at the absolute cap, in every format.
#[test]
fn test_attack_t1_bomb_stops_at_cap_in_every_format() {
    let zeros = vec![0u8; 32 << 20];
    let cap = 1 << 20;
    let limits = Limits::new().with_max_output(cap);
    let raw = deflate::compress_with(&zeros, Level::BEST);
    assert!(
        raw.len() < 64 * 1024,
        "the bomb is small: {} bytes",
        raw.len()
    );
    assert_eq!(
        deflate::decompress_with(&raw, limits).unwrap_err().kind(),
        ErrorKind::OutputLimit
    );
    assert_eq!(
        zlib::decompress_with(&zlib::compress_with(&zeros, Level::BEST), limits)
            .unwrap_err()
            .kind(),
        ErrorKind::OutputLimit
    );
    assert_eq!(
        gzip::decompress_with(&gzip::compress_with(&zeros, Level::BEST), limits)
            .unwrap_err()
            .kind(),
        ErrorKind::OutputLimit
    );
    assert_eq!(
        compress_lib::decompress_with(&gzip::compress(&zeros), limits)
            .unwrap_err()
            .kind(),
        ErrorKind::OutputLimit
    );
    // The streaming engine never writes past the cap into a larger buffer.
    let mut inflater = Inflater::new().with_limits(limits);
    let mut out = vec![0xAAu8; 2 << 20];
    assert_eq!(
        inflater.finish(&raw, &mut out).unwrap_err().kind(),
        ErrorKind::OutputLimit
    );
    assert_eq!(inflater.total_out(), cap);
    assert!(out[cap as usize..].iter().all(|&b| b == 0xAA));
    // And the default cap holds without any configuration.
    let big = deflate::compress_with(&vec![0u8; 80 << 20], Level::BEST);
    assert_eq!(
        deflate::decompress(&big).unwrap_err().kind(),
        ErrorKind::OutputLimit
    );
}

/// T-2: a bomb below the absolute cap is caught by the ratio cap.
#[test]
fn test_attack_t2_ratio_bomb_caught_by_ratio_cap() {
    let packed = gzip::compress(&vec![b'a'; 8 << 20]);
    let limits = Limits::new()
        .with_max_ratio(100)
        .with_ratio_floor(64 * 1024);
    assert_eq!(
        gzip::decompress_with(&packed, limits).unwrap_err().kind(),
        ErrorKind::RatioLimit
    );
    // Ordinary data passes the same limits.
    let mut x = 9u32;
    let text: Vec<u8> = (0..200_000)
        .map(|_| b"etaoin shrdlu\n"[(next(&mut x) % 14) as usize])
        .collect();
    assert_eq!(
        gzip::decompress_with(&gzip::compress(&text), limits).unwrap(),
        text
    );
}

/// T-3: malformed Huffman descriptions are refused, never misread.
#[test]
fn test_attack_t3_malformed_tables_refused() {
    // Dynamic block, HLIT 257, HDIST 1, HCLEN 19, every code-length code
    // length 7: over-subscribed.
    let mut w = Bits::new();
    w.bits(1, 1).bits(2, 2).bits(0, 5).bits(0, 5).bits(15, 4);
    for _ in 0..19 {
        w.bits(7, 3);
    }
    assert_eq!(
        deflate::decompress(&w.done()).unwrap_err().kind(),
        ErrorKind::InvalidCodeLengths
    );
    // All code-length code lengths zero: no code at all.
    let mut w = Bits::new();
    w.bits(1, 1).bits(2, 2).bits(0, 5).bits(0, 5).bits(15, 4);
    for _ in 0..19 {
        w.bits(0, 3);
    }
    w.bits(0, 16);
    assert_eq!(
        deflate::decompress(&w.done()).unwrap_err().kind(),
        ErrorKind::InvalidCodeLengths
    );
    // Random dynamic headers never panic and never decode without error
    // checks firing in a bounded time.
    let mut x = 77u32;
    let start = Instant::now();
    for _ in 0..20_000 {
        let mut data = vec![0x05u8];
        data.extend((0..64).map(|_| next(&mut x) as u8));
        let _ = deflate::decompress(&data);
    }
    assert!(start.elapsed() < Duration::from_secs(30));
}

/// T-4: a back-reference before the data or beyond the window is refused.
#[test]
fn test_attack_t4_distance_beyond_data_or_window() {
    // Fixed block: literal 'a', then length 3 at distance 2.
    let mut w = Bits::new();
    w.bits(1, 1).bits(1, 2);
    w.bits(0b1001_0001u32.reverse_bits() >> 24, 8); // 'a' = 0x30 + 0x61
    w.bits(1u32.reverse_bits() >> 25, 7); // length 3 = symbol 257
    w.bits(1u32.reverse_bits() >> 27, 5); // distance 2 = code 1
    w.bits(0, 7);
    assert_eq!(
        deflate::decompress(&w.done()).unwrap_err().kind(),
        ErrorKind::DistanceTooFar
    );
    // A stream made with a 32 KiB window is refused by a 512-byte one.
    let mut x = 3u32;
    let mut data: Vec<u8> = (0..4000).map(|_| next(&mut x) as u8).collect();
    let copy = data[..1000].to_vec();
    data.extend_from_slice(&copy);
    let packed = deflate::compress(&data);
    let mut small = Inflater::new().with_window_bits(9).unwrap();
    assert_eq!(
        small
            .finish_vec(&packed, &mut Vec::new())
            .unwrap_err()
            .kind(),
        ErrorKind::DistanceTooFar
    );
}

/// T-5: one-byte buffers cost linear time, not quadratic.
#[test]
fn test_attack_t5_tiny_buffers_linear() {
    let mut x = 5u32;
    let text: Vec<u8> = (0..1 << 20)
        .map(|_| b"abcdefgh ijk\n"[(next(&mut x) % 13) as usize])
        .collect();
    let packed = deflate::compress(&text);
    let t = Instant::now();
    let whole = deflate::decompress_with(&packed, Limits::unlimited()).unwrap();
    let one_shot = t.elapsed();
    let t = Instant::now();
    let mut inflater = Inflater::new().with_limits(Limits::unlimited());
    let mut out = Vec::with_capacity(whole.len());
    let mut byte = [0u8; 1];
    let mut input = &packed[..];
    loop {
        let p = inflater.decompress(input, &mut byte).unwrap();
        input = &input[p.consumed..];
        out.extend_from_slice(&byte[..p.produced]);
        if p.status == Status::Done {
            break;
        }
    }
    let tiny = t.elapsed();
    assert_eq!(out, whole);
    // A call per output byte has a fixed cost; quadratic behaviour would be
    // thousands of times slower than one call.
    assert!(
        tiny < one_shot * 400 + Duration::from_secs(2),
        "one shot {one_shot:?}, one-byte buffers {tiny:?}"
    );
}

/// T-6: inputs that produce nothing still cost bounded work.
#[test]
fn test_attack_t6_empty_blocks_and_members_bounded() {
    // A megabyte of empty stored blocks (5 bytes each) and then the end.
    let mut stream = [0x00u8, 0x00, 0x00, 0xFF, 0xFF].repeat(200_000);
    stream.extend_from_slice(&[0x03, 0x00]);
    let t = Instant::now();
    assert_eq!(deflate::decompress(&stream).unwrap(), b"");
    assert!(t.elapsed() < Duration::from_secs(10));
    // Empty gzip members stop at the member cap.
    let empty = gzip::compress(b"");
    let many = empty.repeat(5000);
    assert_eq!(
        gzip::decompress(&many).unwrap_err().kind(),
        ErrorKind::TooManyMembers
    );
}

/// T-7: gzip header fields cannot exhaust memory.
#[test]
fn test_attack_t7_gzip_header_bounded() {
    let mut member = vec![0x1F, 0x8B, 8, 0x08 | 0x10, 0, 0, 0, 0, 0, 255];
    member.extend(std::iter::repeat_n(b'n', 1 << 20));
    member.push(0);
    assert_eq!(
        gzip::decompress(&member).unwrap_err().kind(),
        ErrorKind::HeaderTooLarge
    );
    // An unterminated name streamed in pieces is refused at the cap too.
    let mut dec = gzip::Decoder::new().with_limits(Limits::new().with_max_header(1000));
    let mut out = [0u8; 16];
    let mut err = None;
    for chunk in member.chunks(100) {
        if let Err(e) = dec.decompress(chunk, &mut out) {
            err = Some(e.kind());
            break;
        }
    }
    assert_eq!(err, Some(ErrorKind::HeaderTooLarge));
}

/// T-8: corrupted data never passes the checksums.
#[test]
fn test_attack_t8_checksums_always_verified() {
    let data = b"integrity matters".repeat(50);
    let packed = gzip::compress(&data);
    for i in packed.len() - 8..packed.len() {
        let mut bad = packed.clone();
        bad[i] ^= 0x01;
        let kind = gzip::decompress(&bad).unwrap_err().kind();
        assert!(
            matches!(
                kind,
                ErrorKind::ChecksumMismatch | ErrorKind::LengthMismatch
            ),
            "gzip byte {i}: {kind:?}"
        );
    }
    let packed = zlib::compress(&data);
    for i in packed.len() - 4..packed.len() {
        let mut bad = packed.clone();
        bad[i] ^= 0x01;
        assert_eq!(
            zlib::decompress(&bad).unwrap_err().kind(),
            ErrorKind::ChecksumMismatch,
            "zlib byte {i}"
        );
    }
}

/// T-9: without context takeover, a secret in one message does not change
/// the compressed size of the next (no cross-message compression oracle).
#[test]
fn test_attack_t9_no_takeover_no_cross_message_oracle() {
    let mut tx = Compressor::new(MessageConfig::new()).unwrap();
    let mut sizes = Vec::new();
    for secret in [&b"token=7f3a9c2e"[..], b"token=00000000"] {
        let mut wire = Vec::new();
        tx.compress_message(secret, &mut wire).unwrap();
        wire.clear();
        // The attacker's guess, compressed after the secret.
        tx.compress_message(b"token=7f3a9c2e", &mut wire).unwrap();
        sizes.push(wire.len());
    }
    assert_eq!(
        sizes[0], sizes[1],
        "the guess compressed the same whatever the earlier secret was"
    );
    // With takeover the oracle exists, which is why it is off by default.
    let mut tx = Compressor::new(MessageConfig::new().with_context_takeover(true)).unwrap();
    let mut first = Vec::new();
    tx.compress_message(b"token=7f3a9c2e", &mut first).unwrap();
    let mut guess = Vec::new();
    tx.compress_message(b"token=7f3a9c2e", &mut guess).unwrap();
    assert!(guess.len() < first.len());
}

/// T-10: memory per engine is fixed by configuration, whatever the input.
#[test]
fn test_attack_t10_memory_fixed_per_engine() {
    let config = MessageConfig::new().with_window_bits(9);
    let mut rx = Decompressor::new(config).unwrap();
    let mut tx = Compressor::new(config).unwrap();
    let (rx0, tx0) = (rx.memory_in_use(), tx.memory_in_use());
    let mut x = 1u32;
    for _ in 0..200 {
        let msg: Vec<u8> = (0..(next(&mut x) % 20_000))
            .map(|_| next(&mut x) as u8 % 7)
            .collect();
        let mut wire = Vec::new();
        tx.compress_message(&msg, &mut wire).unwrap();
        let mut out = Vec::new();
        rx.decompress_message(&wire, &mut out, 1 << 20).unwrap();
    }
    assert_eq!((rx.memory_in_use(), tx.memory_in_use()), (rx0, tx0));
    assert!(
        rx0 < 16 * 1024,
        "a decompressor without takeover holds only its tables: {rx0}"
    );
}

/// T-11: arbitrary input never panics (the fuzz targets run this for
/// hours; this is the quick version in every test run).
#[test]
fn test_attack_t11_no_panic_on_arbitrary_input() {
    let mut x = 11u32;
    for len in [0usize, 1, 2, 3, 10, 100, 1000] {
        for _ in 0..300 {
            let data: Vec<u8> = (0..len).map(|_| next(&mut x) as u8).collect();
            let _ = deflate::decompress(&data);
            let _ = zlib::decompress(&data);
            let _ = gzip::decompress(&data);
            let mut rx = Decompressor::new(MessageConfig::new().with_window_bits(8)).unwrap();
            let _ = rx.decompress_message(&data, &mut Vec::new(), 1 << 16);
        }
    }
}

/// T-12: a truncated stream is never reported as complete.
#[test]
fn test_attack_t12_truncation_detected() {
    let data = b"complete or nothing ".repeat(500);
    let raw = deflate::compress(&data);
    let z = zlib::compress(&data);
    let g = gzip::compress(&data);
    for cut in 0..g.len() {
        if cut < raw.len() {
            assert_eq!(
                deflate::decompress(&raw[..cut]).unwrap_err().kind(),
                ErrorKind::UnexpectedEnd,
                "raw {cut}"
            );
        }
        if cut < z.len() {
            assert_eq!(
                zlib::decompress(&z[..cut]).unwrap_err().kind(),
                ErrorKind::UnexpectedEnd,
                "zlib {cut}"
            );
        }
        assert_eq!(
            gzip::decompress(&g[..cut]).unwrap_err().kind(),
            ErrorKind::UnexpectedEnd,
            "gzip {cut}"
        );
    }
}

/// T-13: bytes after the end are refused, not ignored, by default.
#[test]
fn test_attack_t13_trailing_data_refused() {
    let tail = b"SMUGGLED";
    let with_tail = |mut v: Vec<u8>| {
        v.extend_from_slice(tail);
        v
    };
    assert_eq!(
        deflate::decompress(&with_tail(deflate::compress(b"x")))
            .unwrap_err()
            .kind(),
        ErrorKind::TrailingData
    );
    assert_eq!(
        zlib::decompress(&with_tail(zlib::compress(b"x")))
            .unwrap_err()
            .kind(),
        ErrorKind::TrailingData
    );
    assert_eq!(
        gzip::decompress(&with_tail(gzip::compress(b"x")))
            .unwrap_err()
            .kind(),
        ErrorKind::TrailingData
    );
    let mut rx = Decompressor::new(MessageConfig::new()).unwrap();
    let mut final_block = deflate::compress(b"x");
    final_block.push(0x42);
    assert_eq!(
        rx.decompress_message(&final_block, &mut Vec::new(), 64)
            .unwrap_err()
            .kind(),
        ErrorKind::TrailingData
    );
}

/// T-14: input built to make the match finder work hardest stays bounded.
#[test]
fn test_attack_t14_adversarial_encoder_input_bounded() {
    // The same three bytes everywhere, followed by a byte that changes:
    // every chain is full and every candidate matches three bytes.
    let mut x = 13u32;
    let mut data = Vec::new();
    while data.len() < 1 << 20 {
        data.extend_from_slice(b"abc");
        data.push(next(&mut x) as u8);
    }
    let t = Instant::now();
    let six = deflate::compress_with(&data, Level::DEFAULT);
    let level6 = t.elapsed();
    let t = Instant::now();
    let nine = deflate::compress_with(&data, Level::BEST);
    let level9 = t.elapsed();
    assert_eq!(
        deflate::decompress_with(&nine, Limits::unlimited()).unwrap(),
        data
    );
    assert_eq!(
        deflate::decompress_with(&six, Limits::unlimited()).unwrap(),
        data
    );
    // Level 9 searches 32 times deeper than level 6 (4096 against 128); the
    // chain limit keeps it within that factor.
    assert!(
        level9 < level6 * 40 + Duration::from_secs(1),
        "level 6 {level6:?}, level 9 {level9:?}"
    );
    // A deflater fed the same input in small pieces with flushes stays
    // correct and bounded too.
    let mut d = Deflater::new(Level::BEST);
    let mut out = Vec::new();
    for chunk in data.chunks(997) {
        d.compress_vec(chunk, &mut out, Flush::Sync).unwrap();
    }
    d.compress_vec(b"", &mut out, Flush::Finish).unwrap();
    assert_eq!(
        deflate::decompress_with(&out, Limits::unlimited()).unwrap(),
        data
    );
}
