//! Deflate then inflate: any data, level, window, and flush plan comes back
//! unchanged, never uses a distance beyond the window, and zlib accepts it.
//! The same flush plan with the input split into other pieces and the
//! output drained through another buffer size gives the same bytes.

#![no_main]

use compress_lib::{
    Level, Limits,
    deflate::{Deflater, Flush, Inflater, Status},
};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|input: &[u8]| {
    let Some((&[a, b, c], data)) = input.split_first_chunk::<3>().map(|(h, d)| (h, d)) else { return };
    let level = Level::new(a % 10).unwrap();
    let bits = 8 + (b % 8);
    let out_size = 1 + usize::from(c) * 3;
    let mut d = Deflater::new(level).with_window_bits(bits).unwrap();
    let mut packed = Vec::new();
    let mut buf = vec![0u8; out_size];
    // Flush points every few hundred bytes, chosen by the data itself.
    let mut plan = Vec::new();
    let mut pos = 0;
    while pos <= data.len() {
        let step = 1 + usize::from(data.get(pos).copied().unwrap_or(0)) * 4;
        let end = (pos + step).min(data.len());
        let last = end == data.len();
        let flush = if last {
            Flush::Finish
        } else {
            [Flush::None, Flush::None, Flush::Sync, Flush::Full][usize::from(data[pos] & 3)]
        };
        plan.push((pos, end, flush));
        let mut chunk = &data[pos..end];
        loop {
            let p = d.compress(chunk, &mut buf, flush).unwrap();
            chunk = &chunk[p.consumed..];
            packed.extend_from_slice(&buf[..p.produced]);
            if p.status != Status::NeedsOutput {
                break;
            }
        }
        assert!(chunk.is_empty());
        if last {
            break;
        }
        pos = end;
    }
    // The same plan, each segment fed in pieces of 1 to 7 bytes.
    let piece = 1 + usize::from(c % 7);
    let mut again = Vec::new();
    let mut buf = vec![0u8; 1 + usize::from(b) % 5];
    let mut d = Deflater::new(level).with_window_bits(bits).unwrap();
    for &(start, end, flush) in &plan {
        let mut at = start;
        loop {
            let stop = (at + piece).min(end);
            let f = if stop == end { flush } else { Flush::None };
            let mut chunk = &data[at..stop];
            loop {
                let p = d.compress(chunk, &mut buf, f).unwrap();
                chunk = &chunk[p.consumed..];
                again.extend_from_slice(&buf[..p.produced]);
                if p.status != Status::NeedsOutput && chunk.is_empty() {
                    break;
                }
            }
            at = stop;
            if at == end {
                break;
            }
        }
    }
    assert_eq!(again, packed);

    let mut i = Inflater::new().with_window_bits(bits).unwrap().with_limits(Limits::unlimited());
    let mut out = Vec::new();
    let p = i.finish_vec(&packed, &mut out).unwrap();
    assert_eq!(p.consumed, packed.len());
    assert_eq!(out, data);

    let mut z = vec![0u8; data.len() + 1];
    let (got, code) = zlib_rs::decompress_slice(&mut z, &packed, zlib_rs::InflateConfig { window_bits: -15 });
    assert_eq!(code, zlib_rs::ReturnCode::Ok);
    assert_eq!(got, data);
});
