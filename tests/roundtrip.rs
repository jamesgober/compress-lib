//! The deflater: round trips at every level, window size, and flush
//! pattern; other decoders (miniz_oxide, zlib through zlib-rs) accept every
//! stream; window and flush guarantees hold; RFC 7692 vectors.

use compress_lib::{
    Level, Limits,
    deflate::{self, Deflater, Flush, Inflater, Status},
    message::{Compressor, Decompressor, MessageConfig},
};
use proptest::prelude::*;

fn next(x: &mut u32) -> u32 {
    *x ^= *x << 13;
    *x ^= *x >> 17;
    *x ^= *x << 5;
    *x
}

fn corpus() -> Vec<Vec<u8>> {
    let mut x = 0xACE1_u32;
    let words = [
        "alpha", "beta", "gamma", "delta", "compress", "the", "a", "\n", "window", "of",
    ];
    let mut text = Vec::new();
    while text.len() < 150_000 {
        text.extend_from_slice(words[(next(&mut x) % 10) as usize].as_bytes());
        text.push(b' ');
    }
    let random: Vec<u8> = (0..40_000).map(|_| next(&mut x) as u8).collect();
    let mut mixed = Vec::new();
    for i in 0..30_000u32 {
        if i % 7 == 0 {
            mixed.extend_from_slice(&random[(i as usize * 13) % 30_000..][..20]);
        } else {
            mixed.extend_from_slice(&i.to_be_bytes());
        }
    }
    vec![
        Vec::new(),
        vec![42],
        b"abcabcabcabcabc".to_vec(),
        vec![0; 200_000],
        text,
        random,
        mixed,
        (0..=255u8).cycle().take(100_000).collect(),
    ]
}

fn inflate_strict(data: &[u8], bits: u8) -> Vec<u8> {
    let mut i = Inflater::new()
        .with_window_bits(bits)
        .unwrap()
        .with_limits(Limits::unlimited());
    let mut out = Vec::new();
    let p = i.finish_vec(data, &mut out).unwrap();
    assert_eq!(p.consumed, data.len(), "trailing bytes after the stream");
    out
}

fn zlib_inflate(data: &[u8], expect_len: usize) -> Vec<u8> {
    let mut out = vec![0u8; expect_len + 16];
    let (got, code) =
        zlib_rs::decompress_slice(&mut out, data, zlib_rs::InflateConfig { window_bits: -15 });
    assert_eq!(code, zlib_rs::ReturnCode::Ok, "zlib rejects our stream");
    got.to_vec()
}

#[test]
fn test_roundtrip_every_level_and_window() {
    for (i, data) in corpus().iter().enumerate() {
        for level in 0..=9u8 {
            for bits in [8u8, 9, 12, 15] {
                let mut d = Deflater::new(Level::new(level).unwrap())
                    .with_window_bits(bits)
                    .unwrap();
                let mut packed = Vec::new();
                d.compress_vec(data, &mut packed, Flush::Finish).unwrap();
                assert_eq!(
                    &inflate_strict(&packed, bits),
                    data,
                    "corpus {i} level {level} bits {bits}"
                );
                if bits == 15 {
                    assert_eq!(
                        &zlib_inflate(&packed, data.len()),
                        data,
                        "zlib, corpus {i} level {level}"
                    );
                    assert_eq!(
                        &miniz_oxide::inflate::decompress_to_vec(&packed).unwrap(),
                        data,
                        "miniz, corpus {i} level {level}"
                    );
                }
            }
        }
    }
}

#[test]
fn test_roundtrip_ratio_comparable_to_zlib() {
    // Within 1.5% of zlib's size at levels 1, 6, and 9 on text (the 0.6
    // target is 1%; this guards against regressions until then).
    let text = &corpus()[4];
    for level in [1, 6, 9] {
        let ours = deflate::compress_with(text, Level::new(level as u8).unwrap()).len();
        let mut buf = vec![0u8; text.len()];
        let config = zlib_rs::DeflateConfig {
            level,
            window_bits: -15,
            ..Default::default()
        };
        let (z, _) = zlib_rs::compress_slice(&mut buf, text, config);
        let zlen = z.len();
        assert!(
            (ours as f64) <= zlen as f64 * 1.015,
            "level {level}: ours {ours} bytes, zlib {zlen} bytes"
        );
    }
}

#[test]
fn test_roundtrip_empty_stream_is_03_00() {
    assert_eq!(deflate::compress(b""), [0x03, 0x00]);
    assert_eq!(
        deflate::compress_with(b"", Level::NONE),
        [0x01, 0x00, 0x00, 0xFF, 0xFF]
    );
}

#[test]
fn test_roundtrip_level_0_is_stored() {
    let data = &corpus()[4][..70_000];
    let packed = deflate::compress_with(data, Level::NONE);
    // Stored blocks cost 5 bytes per 65,535 bytes at most.
    assert!(packed.len() <= data.len() + 5 * (data.len() / 16_384 + 2));
    assert_eq!(
        deflate::decompress_with(&packed, Limits::unlimited()).unwrap(),
        data
    );
}

#[test]
fn test_roundtrip_sync_flush_ends_with_marker_and_is_decodable() {
    let mut d = Deflater::default();
    let mut out = Vec::new();
    d.compress_vec(b"first part ", &mut out, Flush::Sync)
        .unwrap();
    assert!(out.ends_with(&[0, 0, 0xFF, 0xFF]));
    // Everything so far decodes without the rest of the stream.
    let mut i = Inflater::new();
    let mut got = Vec::new();
    let p = i.decompress_vec(&out, &mut got).unwrap();
    assert_eq!(p.status, Status::NeedsInput);
    assert_eq!(got, b"first part ");
    // A sync flush with nothing new still writes the marker (E-27).
    let n = out.len();
    d.compress_vec(b"", &mut out, Flush::Sync).unwrap();
    assert_eq!(&out[n..], &[0, 0, 0, 0xFF, 0xFF]);
    d.compress_vec(b"and the rest", &mut out, Flush::Finish)
        .unwrap();
    assert_eq!(
        deflate::decompress(&out).unwrap(),
        b"first part and the rest"
    );
}

#[test]
fn test_roundtrip_full_flush_cuts_history() {
    let text = &corpus()[4][..20_000];
    let mut d = Deflater::new(Level::BEST);
    let mut out = Vec::new();
    d.compress_vec(text, &mut out, Flush::Full).unwrap();
    let cut = out.len();
    d.compress_vec(text, &mut out, Flush::Finish).unwrap();
    // The second half decodes alone: nothing reaches back across the flush.
    assert_eq!(deflate::decompress(&out[cut..]).unwrap(), text);
    assert_eq!(deflate::decompress(&out).unwrap().len(), 40_000);
}

#[test]
fn test_roundtrip_tiny_output_buffers() {
    let data = &corpus()[6];
    let mut d = Deflater::new(Level::DEFAULT);
    let mut packed = Vec::new();
    let mut buf = [0u8; 7];
    let mut input = &data[..];
    loop {
        let p = d.compress(input, &mut buf, Flush::Finish).unwrap();
        input = &input[p.consumed..];
        packed.extend_from_slice(&buf[..p.produced]);
        if p.status == Status::Done {
            break;
        }
    }
    assert!(input.is_empty());
    assert_eq!(
        deflate::decompress_with(&packed, Limits::unlimited()).unwrap(),
        *data
    );
    // Done is sticky; more input after Finish is refused.
    assert_eq!(
        d.compress(b"", &mut buf, Flush::Finish).unwrap().status,
        Status::Done
    );
    assert!(d.compress(b"x", &mut buf, Flush::Finish).is_err());
}

#[test]
fn test_roundtrip_output_independent_of_input_split() {
    let data = &corpus()[4][..60_000];
    let whole = deflate::compress_with(data, Level::DEFAULT);
    for step in [1usize, 7, 100, 4096, 30_000] {
        let mut d = Deflater::new(Level::DEFAULT);
        let mut out = Vec::new();
        for chunk in data.chunks(step) {
            d.compress_vec(chunk, &mut out, Flush::None).unwrap();
        }
        d.compress_vec(b"", &mut out, Flush::Finish).unwrap();
        assert_eq!(out, whole, "split {step}");
    }
}

#[test]
fn test_roundtrip_dictionary_shared_by_both_sides() {
    let dict = b"{\"event\":\"price\",\"symbol\":\"";
    let msg = b"{\"event\":\"price\",\"symbol\":\"ACME\",\"value\":12.5}";
    let mut d = Deflater::new(Level::BEST);
    d.set_dictionary(dict).unwrap();
    let mut with = Vec::new();
    d.compress_vec(msg, &mut with, Flush::Finish).unwrap();
    assert!(with.len() < deflate::compress_with(msg, Level::BEST).len());
    let mut i = Inflater::new();
    i.set_dictionary(dict).unwrap();
    let mut out = Vec::new();
    i.finish_vec(&with, &mut out).unwrap();
    assert_eq!(out, msg);
    assert!(d.set_dictionary(dict).is_err());
}

#[test]
fn test_roundtrip_set_level_mid_stream() {
    let data = &corpus()[4][..50_000];
    let mut d = Deflater::new(Level::FAST);
    let mut out = Vec::new();
    for (k, chunk) in data.chunks(5000).enumerate() {
        d.set_level(Level::new((k % 10) as u8).unwrap());
        d.compress_vec(chunk, &mut out, Flush::None).unwrap();
    }
    d.compress_vec(b"", &mut out, Flush::Finish).unwrap();
    assert_eq!(deflate::decompress(&out).unwrap(), data);
}

#[test]
fn test_roundtrip_level_9_adversarial_chains_terminate() {
    // Every position hashes the same three bytes followed by a varying
    // byte: the longest chains a match finder can meet.
    let mut data = Vec::new();
    let mut x = 7u32;
    while data.len() < 200_000 {
        data.extend_from_slice(b"xyz");
        data.push(next(&mut x) as u8);
    }
    let packed = deflate::compress_with(&data, Level::BEST);
    assert_eq!(deflate::decompress(&packed).unwrap(), data);
}

#[test]
fn test_message_rfc7692_vectors() {
    // RFC 7692 7.2.3.1: "Hello" without context takeover.
    let mut tx = Compressor::new(MessageConfig::new()).unwrap();
    let mut wire = Vec::new();
    tx.compress_message(b"Hello", &mut wire).unwrap();
    assert_eq!(wire, [0xF2, 0x48, 0xCD, 0xC9, 0xC9, 0x07, 0x00]);
    wire.clear();
    tx.compress_message(b"Hello", &mut wire).unwrap();
    assert_eq!(wire, [0xF2, 0x48, 0xCD, 0xC9, 0xC9, 0x07, 0x00]);

    // 7.2.3.2: the second "Hello" with takeover is one back-reference.
    let mut tx = Compressor::new(MessageConfig::new().with_context_takeover(true)).unwrap();
    let mut first = Vec::new();
    tx.compress_message(b"Hello", &mut first).unwrap();
    let mut second = Vec::new();
    tx.compress_message(b"Hello", &mut second).unwrap();
    assert_eq!(second, [0xF2, 0x00, 0x11, 0x00, 0x00]);
    let mut rx = Decompressor::new(MessageConfig::new().with_context_takeover(true)).unwrap();
    let mut a = Vec::new();
    rx.decompress_message(&first, &mut a, 100).unwrap();
    let mut b = Vec::new();
    rx.decompress_message(&second, &mut b, 100).unwrap();
    assert_eq!((a.as_slice(), b.as_slice()), (&b"Hello"[..], &b"Hello"[..]));

    // 7.2.3.3: a stored block; 7.2.3.4: BFINAL set; 7.2.3.5: two blocks.
    let mut rx = Decompressor::new(MessageConfig::new()).unwrap();
    for payload in [
        &[
            0x00, 0x05, 0x00, 0xFA, 0xFF, 0x48, 0x65, 0x6C, 0x6C, 0x6F, 0x00,
        ][..],
        &[0xF3, 0x48, 0xCD, 0xC9, 0xC9, 0x07, 0x00, 0x00][..],
        &[
            0xF2, 0x48, 0x05, 0x00, 0x00, 0x00, 0xFF, 0xFF, 0xCA, 0xC9, 0xC9, 0x07, 0x00,
        ][..],
    ] {
        let mut out = Vec::new();
        rx.decompress_message(payload, &mut out, 100).unwrap();
        assert_eq!(out, b"Hello", "{payload:02x?}");
    }
}

#[test]
fn test_message_empty_and_incompressible() {
    let mut tx = Compressor::new(MessageConfig::new()).unwrap();
    let mut rx = Decompressor::new(MessageConfig::new()).unwrap();
    let mut wire = Vec::new();
    tx.compress_message(b"", &mut wire).unwrap();
    assert_eq!(wire, [0x00]);
    let mut out = Vec::new();
    rx.decompress_message(&wire, &mut out, 0).unwrap();
    assert!(out.is_empty());
    // Random bytes compress to more than they are (E-30); still decodable.
    let mut x = 99u32;
    let noise: Vec<u8> = (0..1000).map(|_| next(&mut x) as u8).collect();
    wire.clear();
    let n = tx.compress_message(&noise, &mut wire).unwrap();
    assert!(n > noise.len());
    rx.decompress_message(&wire, &mut out, 2000).unwrap();
    assert_eq!(out, noise);
}

#[test]
fn test_message_budget_checked_during_inflation() {
    let mut tx = Compressor::new(MessageConfig::new()).unwrap();
    let mut wire = Vec::new();
    tx.compress_message(&vec![0u8; 1 << 20], &mut wire).unwrap();
    assert!(wire.len() < 8192);
    let mut rx = Decompressor::new(MessageConfig::new()).unwrap();
    let mut out = Vec::new();
    let err = rx
        .decompress_message(&wire, &mut out, 64 * 1024)
        .unwrap_err();
    assert_eq!(err.kind(), compress_lib::ErrorKind::OutputLimit);
    assert!(out.capacity() <= 64 * 1024);
    // Exactly the budget is fine.
    let mut out = Vec::new();
    rx.decompress_message(&wire, &mut out, 1 << 20).unwrap();
    assert_eq!(out.len(), 1 << 20);
}

#[test]
fn test_message_trailing_after_final_block_rejected() {
    let mut rx = Decompressor::new(MessageConfig::new()).unwrap();
    let mut out = Vec::new();
    // Zero padding after the final block is accepted (RFC 7692 7.2.3.4);
    // anything else is not.
    rx.decompress_message(
        &[0xF3, 0x48, 0xCD, 0xC9, 0xC9, 0x07, 0x00, 0x00, 0x00],
        &mut out,
        100,
    )
    .unwrap();
    assert_eq!(out, b"Hello");
    let err = rx
        .decompress_message(
            &[0xF3, 0x48, 0xCD, 0xC9, 0xC9, 0x07, 0x00, 0x00, 0x42],
            &mut out,
            100,
        )
        .unwrap_err();
    assert_eq!(err.kind(), compress_lib::ErrorKind::TrailingData);
}

#[test]
fn test_message_fragments_and_window_bits_8() {
    let config = MessageConfig::new()
        .with_window_bits(8)
        .with_context_takeover(true);
    let mut tx = Compressor::new(config).unwrap();
    let mut rx = Decompressor::new(config).unwrap();
    let text = &corpus()[4];
    for (k, msg) in text.chunks(3001).take(20).enumerate() {
        let mut wire = Vec::new();
        tx.compress_message(msg, &mut wire).unwrap();
        let mut out = Vec::new();
        let step = 1 + k * 37;
        let pieces: Vec<&[u8]> = wire.chunks(step).collect();
        for (j, piece) in pieces.iter().enumerate() {
            rx.decompress_fragment(piece, j + 1 == pieces.len(), &mut out, 1 << 20)
                .unwrap();
        }
        if pieces.is_empty() {
            rx.decompress_fragment(&[], true, &mut out, 1 << 20)
                .unwrap();
        }
        assert_eq!(out, msg, "message {k}");
    }
}

#[test]
fn test_message_changed_vector_between_fragments_refused() {
    let mut tx = Compressor::new(MessageConfig::new()).unwrap();
    let mut wire = Vec::new();
    tx.compress_message(b"some longer message text", &mut wire)
        .unwrap();
    let mut rx = Decompressor::new(MessageConfig::new()).unwrap();
    let mut out = Vec::new();
    rx.decompress_fragment(&wire[..4], false, &mut out, 100)
        .unwrap();
    out.push(b'!');
    let err = rx
        .decompress_fragment(&wire[4..], true, &mut out, 100)
        .unwrap_err();
    assert_eq!(err.kind(), compress_lib::ErrorKind::InvalidState);
}

#[test]
fn test_message_no_takeover_messages_independent() {
    // Without takeover, every message decodes with a fresh decoder (T-9).
    let mut tx = Compressor::new(MessageConfig::new()).unwrap();
    for msg in [&b"secret=hunter2"[..], b"secret=hunter3", b"secret=hunter2"] {
        let mut wire = Vec::new();
        tx.compress_message(msg, &mut wire).unwrap();
        let mut fresh = Decompressor::new(MessageConfig::new()).unwrap();
        let mut out = Vec::new();
        fresh.decompress_message(&wire, &mut out, 100).unwrap();
        assert_eq!(out, msg);
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(96))]

    #[test]
    fn prop_roundtrip_random_flushes(
        data in proptest::collection::vec(any::<u8>(), 0..20_000),
        level in 0u8..=9,
        bits in 8u8..=15,
        cuts in proptest::collection::vec((any::<u16>(), 0u8..4), 0..12),
        out_size in 1usize..512,
    ) {
        let mut d = Deflater::new(Level::new(level).unwrap()).with_window_bits(bits).unwrap();
        let mut packed = Vec::new();
        let mut buf = vec![0u8; out_size];
        let mut pos = 0;
        let mut plan: Vec<(usize, Flush)> = cuts.iter().map(|&(c, f)| {
            let at = if data.is_empty() { 0 } else { usize::from(c) % (data.len() + 1) };
            (at, [Flush::None, Flush::Sync, Flush::Full, Flush::None][usize::from(f)])
        }).collect();
        plan.sort_by_key(|&(at, _)| at);
        plan.push((data.len(), Flush::Finish));
        for (at, flush) in plan {
            let at = at.max(pos);
            let mut input = &data[pos..at];
            loop {
                let p = d.compress(input, &mut buf, flush).unwrap();
                input = &input[p.consumed..];
                packed.extend_from_slice(&buf[..p.produced]);
                match p.status {
                    Status::NeedsOutput => continue,
                    _ => break,
                }
            }
            prop_assert!(input.is_empty());
            pos = at;
        }
        prop_assert_eq!(inflate_strict(&packed, bits), data);
    }
}

#[test]
fn test_roundtrip_level_0_full_window_never_stalls() {
    // Found by `prop_roundtrip_random_flushes`: at level 0 with a 4 KiB
    // window, flushes left the window full of less than one stored chunk,
    // unable to slide or to write, and `compress` looped.
    let mut x = 5u32;
    let data: Vec<u8> = (0..17_308).map(|_| next(&mut x) as u8).collect();
    for bits in 8..=15 {
        let mut d = Deflater::new(Level::NONE).with_window_bits(bits).unwrap();
        let mut packed = Vec::new();
        let cuts = [
            1285, 3109, 7044, 9359, 10_734, 12_076, 12_439, 12_763, 14_618, 16_760,
        ];
        let mut pos = 0;
        for (k, &at) in cuts.iter().enumerate() {
            let flush = [Flush::None, Flush::Full, Flush::Sync][k % 3];
            d.compress_vec(&data[pos..at], &mut packed, flush).unwrap();
            pos = at;
        }
        d.compress_vec(&data[pos..], &mut packed, Flush::Finish)
            .unwrap();
        assert_eq!(inflate_strict(&packed, bits), data, "bits {bits}");
    }
}
