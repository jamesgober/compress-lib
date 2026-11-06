//! Checksum throughput.

use std::hint::black_box;

use compress_lib::checksum::{adler32, crc32};
use criterion::{Criterion, Throughput, criterion_group, criterion_main};

fn data(len: usize) -> Vec<u8> {
    // A fixed pseudo-random pattern (xorshift), the same on every machine.
    let mut x: u32 = 0x9E37_79B9;
    (0..len)
        .map(|_| {
            x ^= x << 13;
            x ^= x >> 17;
            x ^= x << 5;
            x as u8
        })
        .collect()
}

fn checksums(c: &mut Criterion) {
    for len in [64usize, 4096, 1 << 20] {
        let input = data(len);
        let mut group = c.benchmark_group(format!("checksum/{len}"));
        group.throughput(Throughput::Bytes(len as u64));
        group.bench_function("crc32", |b| b.iter(|| crc32(black_box(&input))));
        group.bench_function("adler32", |b| b.iter(|| adler32(black_box(&input))));
        group.finish();
    }
}

criterion_group!(benches, checksums);
criterion_main!(benches);
