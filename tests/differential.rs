//! Differential tests: compress-lib's inflater against miniz_oxide (output on
//! valid streams of every level) and zlib-rs (zlib's exact accept and reject
//! rules on corrupted and random input).

use compress_lib::{
    ErrorKind,
    deflate::{self, Inflater, Status},
};
use proptest::prelude::*;

/// Deterministic test corpora: text-like, structured binary, random, runs.
fn corpus() -> Vec<Vec<u8>> {
    let mut x: u32 = 0x1234_5678;
    let mut next = move || {
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        x
    };
    let words = [
        "the", "of", "compress", "stream", "window", "block", "huffman", "and", "to", "data", "a",
        "limit", "message", "server", "gzip", "deflate", "inflate", "\n", ", ", ". ",
    ];
    let mut text = Vec::new();
    while text.len() < 200_000 {
        text.extend_from_slice(words[(next() % words.len() as u32) as usize].as_bytes());
        text.push(b' ');
    }
    let mut json = Vec::new();
    for i in 0..3000u32 {
        json.extend_from_slice(
            format!(
                "{{\"id\":{i},\"name\":\"user{}\",\"score\":{},\"ok\":true}}\n",
                next() % 977,
                next() % 100_000
            )
            .as_bytes(),
        );
    }
    let random: Vec<u8> = (0..70_000).map(|_| next() as u8).collect();
    let mut binary = Vec::new();
    for i in 0..20_000u32 {
        binary.extend_from_slice(&i.to_le_bytes());
        binary.extend_from_slice(&((next() % 16) as u16).to_le_bytes());
    }
    vec![
        Vec::new(),
        b"a".to_vec(),
        vec![0u8; 100_000],
        text,
        json,
        random,
        binary,
        (0..=255u8).cycle().take(70_000).collect(),
    ]
}

fn zlib_accepts(data: &[u8]) -> Option<Vec<u8>> {
    let mut out = vec![0u8; 1 << 22];
    let (produced, code) =
        zlib_rs::decompress_slice(&mut out, data, zlib_rs::InflateConfig { window_bits: -15 });
    let n = produced.len();
    if code == zlib_rs::ReturnCode::Ok {
        out.truncate(n);
        Some(out)
    } else {
        None
    }
}

/// Our verdict on a complete stream, ignoring trailing bytes as zlib does.
fn ours_accepts(data: &[u8]) -> Option<Vec<u8>> {
    let mut inflater =
        Inflater::new().with_limits(compress_lib::Limits::new().with_max_output(1 << 22));
    let mut out = Vec::new();
    match inflater.finish_vec(data, &mut out) {
        Ok(p) if p.status == Status::Done => Some(out),
        _ => None,
    }
}

fn chunked(data: &[u8], in_step: usize, out_step: usize) -> Vec<u8> {
    let mut inflater = Inflater::new().with_limits(compress_lib::Limits::unlimited());
    let mut out = Vec::new();
    let mut buf = vec![0u8; out_step];
    let mut pos = 0;
    loop {
        let end = (pos + in_step).min(data.len());
        let p = if end == data.len() {
            inflater.finish(&data[pos..end], &mut buf).unwrap()
        } else {
            inflater.decompress(&data[pos..end], &mut buf).unwrap()
        };
        out.extend_from_slice(&buf[..p.produced]);
        pos += p.consumed;
        if p.status == Status::Done {
            assert_eq!(pos, data.len());
            return out;
        }
    }
}

#[test]
fn test_differential_miniz_every_level_decodes_identically() {
    for (i, data) in corpus().iter().enumerate() {
        for level in 0..=10u8 {
            let packed = miniz_oxide::deflate::compress_to_vec(data, level);
            let ours = deflate::decompress_with(&packed, compress_lib::Limits::unlimited())
                .unwrap_or_else(|e| panic!("corpus {i} level {level}: {e}"));
            assert_eq!(&ours, data, "corpus {i} level {level}");
            assert_eq!(
                &chunked(&packed, 4093, 1000),
                data,
                "corpus {i} level {level} chunked"
            );
        }
    }
}

#[test]
fn test_differential_zlib_rs_every_level_decodes_identically() {
    for (i, data) in corpus().iter().enumerate() {
        for level in [1, 6, 9] {
            let mut packed = vec![0u8; data.len() * 2 + 1024];
            let config = zlib_rs::DeflateConfig {
                level,
                window_bits: -15,
                ..Default::default()
            };
            let (out, code) = zlib_rs::compress_slice(&mut packed, data, config);
            assert_eq!(code, zlib_rs::ReturnCode::Ok);
            let n = out.len();
            packed.truncate(n);
            assert_eq!(
                &deflate::decompress_with(&packed, compress_lib::Limits::unlimited()).unwrap(),
                data
            );
            assert_eq!(
                &chunked(&packed, 1, 7),
                data,
                "corpus {i} level {level} bytewise"
            );
        }
    }
}

#[test]
fn test_differential_small_windows_decode() {
    let data = &corpus()[3];
    for bits in 9..=15 {
        let mut packed = vec![0u8; data.len() * 2];
        let config = zlib_rs::DeflateConfig {
            level: 6,
            window_bits: -bits,
            ..Default::default()
        };
        let (out, code) = zlib_rs::compress_slice(&mut packed, data, config);
        assert_eq!(code, zlib_rs::ReturnCode::Ok);
        let n = out.len();
        packed.truncate(n);
        let mut inflater = Inflater::new()
            .with_window_bits(bits as u8)
            .unwrap()
            .with_limits(compress_lib::Limits::unlimited());
        let mut got = Vec::new();
        inflater.finish_vec(&packed, &mut got).unwrap();
        assert_eq!(&got, data, "window bits {bits}");
    }
}

#[test]
fn test_differential_corrupted_streams_agree_with_zlib() {
    let mut x: u32 = 0xC0FF_EE11;
    let mut next = move || {
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        x
    };
    let mut checked = 0;
    for data in corpus().iter().filter(|d| !d.is_empty()) {
        let sample = &data[..data.len().min(4000)];
        for level in [1u8, 6, 9] {
            let packed = miniz_oxide::deflate::compress_to_vec(sample, level);
            for _ in 0..150 {
                let mut bad = packed.clone();
                let flips = 1 + next() % 3;
                for _ in 0..flips {
                    let i = (next() as usize) % bad.len();
                    bad[i] ^= 1 << (next() % 8);
                }
                let z = zlib_accepts(&bad);
                let o = ours_accepts(&bad);
                assert_eq!(
                    o,
                    z,
                    "verdict or output differs from zlib on {:02x?}",
                    &bad[..bad.len().min(32)]
                );
                checked += 1;
            }
        }
    }
    assert!(checked > 1000);
}

#[test]
fn test_differential_truncation_is_unexpected_end() {
    let data = &corpus()[4];
    let packed = miniz_oxide::deflate::compress_to_vec(data, 6);
    for n in (0..packed.len()).step_by(97) {
        let e = deflate::decompress(&packed[..n]).unwrap_err();
        assert_eq!(e.kind(), ErrorKind::UnexpectedEnd, "prefix {n}");
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(512))]

    #[test]
    fn prop_differential_random_bytes_agree_with_zlib(data in proptest::collection::vec(any::<u8>(), 0..300)) {
        prop_assert_eq!(ours_accepts(&data), zlib_accepts(&data));
    }

    #[test]
    fn prop_differential_round_trip_any_split(
        data in proptest::collection::vec(any::<u8>(), 0..5000),
        level in 0u8..=10,
        in_step in 1usize..300,
        out_step in 1usize..300,
    ) {
        let packed = miniz_oxide::deflate::compress_to_vec(&data, level);
        prop_assert_eq!(chunked(&packed, in_step, out_step), data);
    }
}
