//! Raw DEFLATE compression throughput and size, against miniz_oxide and
//! zlib-rs (the dev-dependency oracles) on the same inputs. The full
//! competitor set is in `bench-compare/` from 0.6.

use std::hint::black_box;

use compress_lib::{Level, deflate};
use criterion::{Criterion, Throughput, criterion_group, criterion_main};

fn next(x: &mut u32) -> u32 {
    *x ^= *x << 13;
    *x ^= *x >> 17;
    *x ^= *x << 5;
    *x
}

/// Text from a fixed word list, the same on every machine.
fn text(len: usize) -> Vec<u8> {
    let words = [
        "the",
        "of",
        "and",
        "to",
        "in",
        "a",
        "is",
        "that",
        "for",
        "it",
        "as",
        "was",
        "with",
        "be",
        "by",
        "on",
        "not",
        "he",
        "this",
        "are",
        "or",
        "his",
        "from",
        "at",
        "which",
        "but",
        "have",
        "an",
        "had",
        "they",
        "you",
        "were",
        "their",
        "one",
        "all",
        "we",
        "can",
        "her",
        "has",
        "there",
        "been",
        "if",
        "more",
        "when",
        "will",
        "would",
        "who",
        "so",
        "no",
        "compression",
    ];
    let mut x = 0x2545_F491;
    let mut out = Vec::with_capacity(len + 16);
    while out.len() < len {
        out.extend_from_slice(words[(next(&mut x) as usize) % words.len()].as_bytes());
        out.push(if next(&mut x) % 13 == 0 { b'\n' } else { b' ' });
    }
    out.truncate(len);
    out
}

/// JSON-like records.
fn json(len: usize) -> Vec<u8> {
    let mut x = 0x9E37_79B9;
    let mut out = Vec::with_capacity(len + 128);
    let mut i = 0u32;
    while out.len() < len {
        out.extend_from_slice(
            format!(
                "{{\"id\":{i},\"user\":\"u{}\",\"score\":{},\"tags\":[\"a\",\"b{}\"],\"ok\":{}}}\n",
                next(&mut x) % 5000,
                next(&mut x) % 100_000,
                next(&mut x) % 9,
                next(&mut x) % 2 == 0
            )
            .as_bytes(),
        );
        i += 1;
    }
    out.truncate(len);
    out
}

fn zlib(data: &[u8], level: i32) -> Vec<u8> {
    let mut out = vec![0u8; data.len() + 1024];
    let config = zlib_rs::DeflateConfig {
        level,
        window_bits: -15,
        ..Default::default()
    };
    let n = zlib_rs::compress_slice(&mut out, data, config).0.len();
    out.truncate(n);
    out
}

fn deflate(c: &mut Criterion) {
    let len = 1 << 20;
    for (name, data) in [("text", text(len)), ("json", json(len))] {
        for level in [1u8, 6, 9] {
            let ours = deflate::compress_with(&data, Level::new(level).unwrap()).len();
            let miniz = miniz_oxide::deflate::compress_to_vec(&data, level).len();
            let z = zlib(&data, i32::from(level)).len();
            println!(
                "size {name} level {level}: compress-lib {ours}, miniz_oxide {miniz}, zlib-rs {z}"
            );
            let mut group = c.benchmark_group(format!("deflate/{name}/level{level}"));
            group.throughput(Throughput::Bytes(data.len() as u64));
            group.sample_size(10);
            group.bench_function("compress-lib", |b| {
                b.iter(|| deflate::compress_with(black_box(&data), Level::new(level).unwrap()))
            });
            group.bench_function("miniz_oxide", |b| {
                b.iter(|| miniz_oxide::deflate::compress_to_vec(black_box(&data), level))
            });
            group.bench_function("zlib-rs", |b| {
                b.iter(|| zlib(black_box(&data), i32::from(level)))
            });
            group.finish();
        }
    }
}

criterion_group!(benches, deflate);
criterion_main!(benches);
