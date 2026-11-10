//! Streaming: any input split and output buffer size gives the output of one
//! call, or the same error.

#![no_main]

use compress_lib::{Limits, deflate::{Inflater, Status}};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let Some((&shape, stream)) = data.split_first() else { return };
    let in_step = 1 + usize::from(shape & 0x0F) * 3;
    // Output buffers on both sides of the fast loop's 274-byte margin, so
    // calls start in the fast loop with history from earlier calls (the
    // 0.9 range stopped at 256 bytes and never got there).
    let out_step = [1, 2, 7, 64, 255, 273, 274, 300, 512, 1000, 2048, 4096, 8191, 16_384, 32_768, 70_000]
        [usize::from(shape >> 4)];
    let limits = Limits::new().with_max_output(1 << 20);

    let mut whole = Inflater::new().with_limits(limits);
    let mut expect = Vec::new();
    let one = whole.finish_vec(stream, &mut expect).map(|p| p.consumed);

    let mut inflater = Inflater::new().with_limits(limits);
    let mut got = Vec::new();
    let mut buf = vec![0u8; out_step];
    let mut pos = 0;
    let pieces = loop {
        let end = (pos + in_step).min(stream.len());
        let r = if end == stream.len() {
            inflater.finish(&stream[pos..end], &mut buf)
        } else {
            inflater.decompress(&stream[pos..end], &mut buf)
        };
        match r {
            Ok(p) => {
                assert!(p.consumed <= end - pos && p.produced <= buf.len());
                got.extend_from_slice(&buf[..p.produced]);
                pos += p.consumed;
                match p.status {
                    Status::Done => break Ok(pos),
                    Status::NeedsInput => assert_eq!(pos, end),
                    _ => {}
                }
            }
            Err(e) => break Err(e.kind()),
        }
    };
    match (one, pieces) {
        (Ok(a), Ok(b)) => {
            assert_eq!(a, b);
            assert_eq!(expect, got);
        }
        (Err(a), Err(b)) => {
            assert_eq!(a.kind(), b);
            assert!(got.starts_with(&expect) || expect.starts_with(&got));
        }
        (a, b) => panic!("one call {a:?}, pieces {b:?}"),
    }
});
