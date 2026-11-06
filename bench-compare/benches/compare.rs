//! compress-lib against miniz_oxide, zlib-rs, and flate2 (one backend per
//! build) on deterministic corpora. Prints compressed sizes, then criterion
//! throughput in uncompressed bytes per second.
//!
//! ```text
//! cargo bench                                   # compress-lib, miniz_oxide, zlib-rs
//! cargo bench --features flate2-zlib            # plus flate2 over C zlib
//! ```

use std::hint::black_box;

use compress_lib::{Level, Limits, deflate, gzip, message};
use criterion::{Criterion, Throughput, criterion_group, criterion_main};

fn next(x: &mut u32) -> u32 {
    *x ^= *x << 13;
    *x ^= *x >> 17;
    *x ^= *x << 5;
    *x
}

/// English-like text from a fixed word list, common words more often.
fn text(len: usize) -> Vec<u8> {
    let words = [
        "the", "of", "and", "to", "in", "a", "is", "that", "for", "it", "as", "was", "with", "be",
        "by", "on", "not", "he", "this", "are", "or", "his", "from", "at", "which", "but", "have",
        "an", "had", "they", "you", "were", "their", "one", "all", "we", "can", "her", "has",
        "there", "been", "if", "more", "when", "will", "would", "who", "so", "no", "compression",
        "window", "stream", "decoder", "message", "server", "request", "response", "header",
    ];
    let mut x = 0x2545_F491;
    let mut out = Vec::with_capacity(len + 16);
    while out.len() < len {
        let r = next(&mut x) as usize;
        let i = (r % words.len()).min(r % 17 + (r >> 8) % 7);
        out.extend_from_slice(words[i % words.len()].as_bytes());
        out.push(match next(&mut x) % 16 {
            0 => b'\n',
            1 => b',',
            2 => b'.',
            _ => b' ',
        });
    }
    out.truncate(len);
    out
}

/// JSON API records.
fn json(len: usize) -> Vec<u8> {
    let mut x = 0x9E37_79B9;
    let mut out = Vec::with_capacity(len + 160);
    let mut i = 0u32;
    while out.len() < len {
        out.extend_from_slice(
            format!(
                "{{\"id\":{i},\"user\":\"u{}\",\"score\":{},\"tags\":[\"a\",\"b{}\"],\"active\":{},\"ts\":\"2026-11-{:02}T{:02}:{:02}:00Z\"}}\n",
                next(&mut x) % 5000,
                next(&mut x) % 100_000,
                next(&mut x) % 9,
                next(&mut x) % 2 == 0,
                1 + next(&mut x) % 28,
                next(&mut x) % 24,
                next(&mut x) % 60
            )
            .as_bytes(),
        );
        i += 1;
    }
    out.truncate(len);
    out
}

/// HTML-like markup.
fn html(len: usize) -> Vec<u8> {
    let mut x = 0xC0DE_CAFE;
    let tags = ["div", "span", "p", "a", "li", "td", "section"];
    let mut out = Vec::with_capacity(len + 200);
    while out.len() < len {
        let t = tags[(next(&mut x) % 7) as usize];
        out.extend_from_slice(
            format!(
                "<{t} class=\"c{}\" id=\"n{}\">item {} of the list</{t}>\n",
                next(&mut x) % 12,
                next(&mut x) % 10_000,
                next(&mut x) % 500
            )
            .as_bytes(),
        );
    }
    out.truncate(len);
    out
}

/// Structured binary: counters, small fields, some noise.
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

fn random(len: usize) -> Vec<u8> {
    let mut x = 0x0BAD_F00D;
    (0..len).map(|_| next(&mut x) as u8).collect()
}

fn corpora() -> Vec<(&'static str, Vec<u8>)> {
    let len = 1 << 20;
    vec![
        ("text", text(len)),
        ("json", json(len)),
        ("html", html(len)),
        ("binary", binary(len)),
        ("random", random(len)),
    ]
}

fn zlib_rs_deflate(data: &[u8], level: i32) -> Vec<u8> {
    let mut out = vec![0u8; data.len() + data.len() / 8 + 1024];
    let config = zlib_rs::DeflateConfig { level, window_bits: -15, ..Default::default() };
    let n = zlib_rs::compress_slice(&mut out, data, config).0.len();
    out.truncate(n);
    out
}

fn zlib_rs_inflate(data: &[u8], len: usize) -> Vec<u8> {
    let mut out = vec![0u8; len];
    let n = zlib_rs::decompress_slice(&mut out, data, zlib_rs::InflateConfig { window_bits: -15 }).0.len();
    out.truncate(n);
    out
}

#[cfg(feature = "flate2")]
fn flate2_deflate(data: &[u8], level: u32) -> Vec<u8> {
    use std::io::Write;
    let mut e = flate2::write::DeflateEncoder::new(Vec::with_capacity(data.len() / 2), flate2::Compression::new(level));
    e.write_all(data).unwrap();
    e.finish().unwrap()
}

#[cfg(feature = "flate2")]
fn flate2_inflate(data: &[u8], len: usize) -> Vec<u8> {
    use std::io::Read;
    let mut out = Vec::with_capacity(len);
    flate2::read::DeflateDecoder::new(data).read_to_end(&mut out).unwrap();
    out
}

fn sizes() {
    println!("compressed sizes in bytes (1 MiB inputs)");
    for (name, data) in corpora() {
        for level in [1u8, 6, 9] {
            let ours = deflate::compress_with(&data, Level::new(level).unwrap()).len();
            let miniz = miniz_oxide::deflate::compress_to_vec(&data, level).len();
            let z = zlib_rs_deflate(&data, i32::from(level)).len();
            #[cfg(feature = "flate2")]
            let f = flate2_deflate(&data, u32::from(level)).len();
            #[cfg(not(feature = "flate2"))]
            let f = 0;
            println!(
                "size {name:<6} level {level}: compress-lib {ours:>8}  miniz_oxide {miniz:>8}  zlib-rs {z:>8}  flate2 {f:>8}"
            );
        }
    }
}

fn compress_benches(c: &mut Criterion) {
    sizes();
    for (name, data) in corpora() {
        for level in [1u8, 6, 9] {
            let mut group = c.benchmark_group(format!("deflate/{name}/level{level}"));
            group.throughput(Throughput::Bytes(data.len() as u64));
            group.sample_size(10);
            let lv = Level::new(level).unwrap();
            group.bench_function("compress-lib", |b| b.iter(|| deflate::compress_with(black_box(&data), lv)));
            group.bench_function("miniz_oxide", |b| {
                b.iter(|| miniz_oxide::deflate::compress_to_vec(black_box(&data), level))
            });
            group.bench_function("zlib-rs", |b| b.iter(|| zlib_rs_deflate(black_box(&data), i32::from(level))));
            #[cfg(feature = "flate2")]
            group.bench_function("flate2", |b| b.iter(|| flate2_deflate(black_box(&data), u32::from(level))));
            group.finish();
        }
    }
}

fn decompress_benches(c: &mut Criterion) {
    for (name, data) in corpora() {
        for level in [1u8, 6] {
            let packed = zlib_rs_deflate(&data, i32::from(level));
            let mut group = c.benchmark_group(format!("inflate/{name}/level{level}"));
            group.throughput(Throughput::Bytes(data.len() as u64));
            group.bench_function("compress-lib", |b| {
                b.iter(|| deflate::decompress_with(black_box(&packed), Limits::unlimited()))
            });
            group.bench_function("miniz_oxide", |b| {
                b.iter(|| miniz_oxide::inflate::decompress_to_vec(black_box(&packed)))
            });
            group.bench_function("zlib-rs", |b| b.iter(|| zlib_rs_inflate(black_box(&packed), data.len())));
            #[cfg(feature = "flate2")]
            group.bench_function("flate2", |b| b.iter(|| flate2_inflate(black_box(&packed), data.len())));
            group.finish();
        }
    }
}

fn gzip_benches(c: &mut Criterion) {
    let data = json(1 << 20);
    let packed = gzip::compress(&data);
    let mut group = c.benchmark_group("gzip/json");
    group.throughput(Throughput::Bytes(data.len() as u64));
    group.bench_function("compress-lib/compress", |b| b.iter(|| gzip::compress(black_box(&data))));
    group.bench_function("compress-lib/decompress", |b| {
        b.iter(|| gzip::decompress_with(black_box(&packed), Limits::unlimited()))
    });
    group.finish();
}

fn message_benches(c: &mut Criterion) {
    // permessage-deflate: small JSON messages, no context takeover, level 1.
    let data = json(64 * 1024);
    let messages: Vec<&[u8]> = data.split(|&b| b == b'\n').filter(|m| !m.is_empty()).take(200).collect();
    let total: usize = messages.iter().map(|m| m.len()).sum();
    let mut group = c.benchmark_group("message/json-200");
    group.throughput(Throughput::Bytes(total as u64));
    let config = message::MessageConfig::new();
    group.bench_function("compress-lib", |b| {
        let mut tx = message::Compressor::new(config).unwrap();
        let mut wire = Vec::with_capacity(1024);
        b.iter(|| {
            for m in &messages {
                wire.clear();
                tx.compress_message(black_box(m), &mut wire).unwrap();
            }
        })
    });
    group.bench_function("miniz_oxide", |b| {
        use miniz_oxide::deflate::core::{CompressorOxide, TDEFLFlush, compress, create_comp_flags_from_zip_params};
        let mut state = CompressorOxide::new(create_comp_flags_from_zip_params(1, -15, 0));
        let mut wire = vec![0u8; 4096];
        b.iter(|| {
            for m in &messages {
                state.reset();
                let _ = compress(&mut state, black_box(m), &mut wire, TDEFLFlush::Sync);
            }
        })
    });
    group.finish();
}

criterion_group!(benches, compress_benches, decompress_benches, gzip_benches, message_benches);
criterion_main!(benches);
