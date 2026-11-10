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
use compress_lib_bench_compare::{corpora, json};
use criterion::{Criterion, Throughput, criterion_group, criterion_main};

fn zlib_rs_deflate(data: &[u8], level: i32) -> Vec<u8> {
    let mut out = vec![0u8; data.len() + data.len() / 8 + 1024];
    let config = zlib_rs::DeflateConfig {
        level,
        window_bits: -15,
        ..Default::default()
    };
    let n = zlib_rs::compress_slice(&mut out, data, config).0.len();
    out.truncate(n);
    out
}

fn zlib_rs_inflate(data: &[u8], len: usize) -> Vec<u8> {
    let mut out = vec![0u8; len];
    let n = zlib_rs::decompress_slice(&mut out, data, zlib_rs::InflateConfig { window_bits: -15 })
        .0
        .len();
    out.truncate(n);
    out
}

#[cfg(feature = "flate2")]
fn flate2_deflate(data: &[u8], level: u32) -> Vec<u8> {
    use std::io::Write;
    let mut e = flate2::write::DeflateEncoder::new(
        Vec::with_capacity(data.len() / 2),
        flate2::Compression::new(level),
    );
    e.write_all(data).unwrap();
    e.finish().unwrap()
}

#[cfg(feature = "flate2")]
fn flate2_inflate(data: &[u8], len: usize) -> Vec<u8> {
    use std::io::Read;
    let mut out = Vec::with_capacity(len);
    flate2::read::DeflateDecoder::new(data)
        .read_to_end(&mut out)
        .unwrap();
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
            group.bench_function("compress-lib", |b| {
                b.iter(|| deflate::compress_with(black_box(&data), lv))
            });
            group.bench_function("miniz_oxide", |b| {
                b.iter(|| miniz_oxide::deflate::compress_to_vec(black_box(&data), level))
            });
            group.bench_function("zlib-rs", |b| {
                b.iter(|| zlib_rs_deflate(black_box(&data), i32::from(level)))
            });
            #[cfg(feature = "flate2")]
            group.bench_function("flate2", |b| {
                b.iter(|| flate2_deflate(black_box(&data), u32::from(level)))
            });
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
            group.bench_function("zlib-rs", |b| {
                b.iter(|| zlib_rs_inflate(black_box(&packed), data.len()))
            });
            #[cfg(feature = "flate2")]
            group.bench_function("flate2", |b| {
                b.iter(|| flate2_inflate(black_box(&packed), data.len()))
            });
            group.finish();
        }
    }
}

fn gzip_benches(c: &mut Criterion) {
    let data = json(1 << 20);
    let packed = gzip::compress(&data);
    let mut group = c.benchmark_group("gzip/json");
    group.throughput(Throughput::Bytes(data.len() as u64));
    group.bench_function("compress-lib/compress", |b| {
        b.iter(|| gzip::compress(black_box(&data)))
    });
    group.bench_function("compress-lib/decompress", |b| {
        b.iter(|| gzip::decompress_with(black_box(&packed), Limits::unlimited()))
    });
    group.finish();
}

fn message_benches(c: &mut Criterion) {
    // permessage-deflate: small JSON messages, no context takeover, level 1.
    let data = json(64 * 1024);
    let messages: Vec<&[u8]> = data
        .split(|&b| b == b'\n')
        .filter(|m| !m.is_empty())
        .take(200)
        .collect();
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
        use miniz_oxide::deflate::core::{
            CompressorOxide, TDEFLFlush, compress, create_comp_flags_from_zip_params,
        };
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

criterion_group!(
    benches,
    compress_benches,
    decompress_benches,
    gzip_benches,
    message_benches
);
criterion_main!(benches);
