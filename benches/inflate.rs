//! Raw DEFLATE decoding throughput, against miniz_oxide (the dev-dependency
//! oracle) on the same streams. The full competitor set is in
//! `bench-compare/` from 0.6.

use std::hint::black_box;

use compress_lib::{Limits, deflate};
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

/// Structured binary: counters and small random fields.
fn binary(len: usize) -> Vec<u8> {
    let mut x = 0xDEAD_BEEF;
    let mut out = Vec::with_capacity(len + 8);
    let mut i = 0u32;
    while out.len() < len {
        out.extend_from_slice(&i.to_le_bytes());
        out.extend_from_slice(&((next(&mut x) % 64) as u16).to_le_bytes());
        out.push(next(&mut x) as u8);
        i += 1;
    }
    out.truncate(len);
    out
}

fn inflate(c: &mut Criterion) {
    let len = 1 << 20;
    let corpora = [
        ("text", text(len)),
        ("json", json(len)),
        ("binary", binary(len)),
    ];
    for (name, data) in &corpora {
        for level in [1u8, 6, 9] {
            let packed = miniz_oxide::deflate::compress_to_vec(data, level);
            let mut group = c.benchmark_group(format!("inflate/{name}/level{level}"));
            group.throughput(Throughput::Bytes(data.len() as u64));
            group.bench_function("compress-lib", |b| {
                b.iter(|| deflate::decompress_with(black_box(&packed), Limits::unlimited()))
            });
            group.bench_function("miniz_oxide", |b| {
                b.iter(|| miniz_oxide::inflate::decompress_to_vec(black_box(&packed)))
            });
            group.finish();
        }
    }
}

criterion_group!(benches, inflate);
criterion_main!(benches);
