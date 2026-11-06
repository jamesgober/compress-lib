//! RFC 7692 messages: compressed messages round trip with and without
//! takeover; arbitrary payloads never panic and never pass the budget.

#![no_main]

use compress_lib::message::{Compressor, Decompressor, MessageConfig};
use libfuzzer_sys::fuzz_target;

const BUDGET: usize = 1 << 18;

fuzz_target!(|input: &[u8]| {
    let Some((&flags, rest)) = input.split_first() else { return };
    let config = MessageConfig::new()
        .with_window_bits(8 + (flags & 7))
        .with_context_takeover(flags & 8 != 0);

    // Arbitrary bytes as received payloads, split into fragments.
    let mut rx = Decompressor::new(config).unwrap();
    for payload in rest.split(|&b| b == 0xA5) {
        let mut out = Vec::new();
        let mut ok = true;
        let pieces: Vec<&[u8]> = payload.chunks(1 + usize::from(flags >> 4)).collect();
        for (j, piece) in pieces.iter().enumerate() {
            match rx.decompress_fragment(piece, j + 1 == pieces.len(), &mut out, BUDGET) {
                Ok(_) => {}
                Err(_) => {
                    ok = false;
                    break;
                }
            }
        }
        assert!(out.len() <= BUDGET);
        if !ok {
            rx.reset();
        }
    }

    // Messages cut from the input round trip through a fresh pair.
    let mut tx = Compressor::new(config).unwrap();
    let mut rx = Decompressor::new(config).unwrap();
    for msg in rest.split(|&b| b == 0x5A) {
        let mut wire = Vec::new();
        tx.compress_message(msg, &mut wire).unwrap();
        let mut out = Vec::new();
        rx.decompress_message(&wire, &mut out, msg.len()).unwrap();
        assert_eq!(out, msg);
    }
});
