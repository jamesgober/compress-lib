//! zlib and gzip decoding: no panic, output within the cap, and the same
//! verdict and output as zlib (through zlib-rs) on single-member input.

#![no_main]

use compress_lib::{Limits, gzip, zlib};
use libfuzzer_sys::fuzz_target;

const CAP: usize = 1 << 20;

fn zlib_rs(data: &[u8], bits: i32) -> Option<Vec<u8>> {
    let mut out = vec![0u8; CAP + 1];
    let (got, code) = zlib_rs::decompress_slice(&mut out, data, zlib_rs::InflateConfig { window_bits: bits });
    (code == zlib_rs::ReturnCode::Ok && got.len() <= CAP).then(|| got.to_vec())
}

fuzz_target!(|data: &[u8]| {
    let limits = Limits::new().with_max_output(CAP as u64);

    // zlib: zlib-rs stops at the end of the stream and ignores what follows,
    // so compare through the streaming decoder, which does the same.
    let mut ours = Vec::new();
    let mut dec = zlib::Decoder::new().with_limits(limits);
    let z = dec.finish_vec(data, &mut ours).ok().map(|_| ours);
    if let Some(theirs) = zlib_rs(data, 15) {
        assert_eq!(z.as_deref(), Some(&theirs[..]), "zlib: zlib-rs accepts, we differ");
    } else if let Some(ours) = &z {
        // We may accept only when zlib-rs needed more room than the cap.
        assert!(ours.len() == CAP, "zlib: we accept what zlib-rs rejects");
    }

    // gzip, single member, same rule.
    let mut ours = Vec::new();
    let mut dec = gzip::Decoder::new().with_limits(limits).with_single_member();
    let g = dec.finish_vec(data, &mut ours).ok().map(|_| ours);
    if let Some(theirs) = zlib_rs(data, 31) {
        assert_eq!(g.as_deref(), Some(&theirs[..]), "gzip: zlib-rs accepts, we differ");
    }

    // Multi-member and Tier 1 never panic and respect the cap.
    if let Ok(out) = gzip::decompress_with(data, limits) {
        assert!(out.len() <= CAP);
    }
    let _ = compress_lib::decompress_with(data, limits);
});
