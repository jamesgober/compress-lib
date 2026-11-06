//! Raw DEFLATE: no panic, output within the cap, and the same verdict and
//! output as zlib (through zlib-rs) on every input.

#![no_main]

use compress_lib::{Limits, deflate::{Inflater, Status}};
use libfuzzer_sys::fuzz_target;

const CAP: usize = 1 << 20;

fuzz_target!(|data: &[u8]| {
    let mut inflater = Inflater::new().with_limits(Limits::new().with_max_output(CAP as u64));
    let mut ours = Vec::new();
    let result = inflater.finish_vec(data, &mut ours);
    assert!(ours.len() <= CAP);
    let ours = match result {
        Ok(p) if p.status == Status::Done => Some(ours),
        _ => None,
    };

    let mut out = vec![0u8; CAP + 1];
    let (produced, code) = zlib_rs::decompress_slice(&mut out, data, zlib_rs::InflateConfig { window_bits: -15 });
    let n = produced.len();
    let zlib = (code == zlib_rs::ReturnCode::Ok && n <= CAP).then(|| out[..n].to_vec());
    // zlib may need more than the cap (our OutputLimit); compare only within it.
    if n <= CAP {
        assert_eq!(ours, zlib, "verdict or output differs from zlib");
    }
});
