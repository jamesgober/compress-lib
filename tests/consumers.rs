//! The consumer paths, end to end, called the way ws-net, http-net, and
//! serve-net call them.

use compress_lib::{
    Decoder, Encoder, ErrorKind, Format, Level, Limits,
    deflate::{Flush, Status},
    message::{Compressor, Decompressor, MessageConfig},
};

fn next(x: &mut u32) -> u32 {
    *x ^= *x << 13;
    *x ^= *x >> 17;
    *x ^= *x << 5;
    *x
}

fn json_message(x: &mut u32) -> Vec<u8> {
    let n = next(x) % 40;
    let mut m = format!(
        "{{\"type\":\"update\",\"seq\":{},\"items\":[",
        next(x) % 100_000
    );
    for i in 0..n {
        m.push_str(&format!(
            "{{\"id\":{i},\"price\":{}.{:02},\"sym\":\"S{}\"}},",
            next(x) % 500,
            next(x) % 100,
            next(x) % 30
        ));
    }
    m.push_str("]}");
    m.into_bytes()
}

/// ws-net: every negotiated parameter combination, messages split into
/// frames at random points, a per-message budget, both directions.
#[test]
fn test_consumer_ws_net_every_negotiation_and_fragmentation() {
    let mut x = 0xA11CE;
    for server_bits in [8u8, 9, 12, 15] {
        for client_bits in [8u8, 10, 15] {
            for (server_takeover, client_takeover) in
                [(false, false), (true, false), (false, true), (true, true)]
            {
                let s2c = MessageConfig::new()
                    .with_window_bits(server_bits)
                    .with_context_takeover(server_takeover);
                let c2s = MessageConfig::new()
                    .with_window_bits(client_bits)
                    .with_context_takeover(client_takeover);
                let mut server_tx = Compressor::new(s2c).unwrap();
                let mut client_rx = Decompressor::new(s2c).unwrap();
                let mut client_tx = Compressor::new(c2s).unwrap();
                let mut server_rx = Decompressor::new(c2s).unwrap();
                for _ in 0..12 {
                    for (tx, rx) in [
                        (&mut server_tx, &mut client_rx),
                        (&mut client_tx, &mut server_rx),
                    ] {
                        let msg = json_message(&mut x);
                        let mut payload = Vec::new();
                        tx.compress_message(&msg, &mut payload).unwrap();
                        // Frames of random sizes; the last carries FIN.
                        let mut out = Vec::new();
                        let mut pos = 0;
                        loop {
                            let len = (1 + next(&mut x) as usize % 300).min(payload.len() - pos);
                            let last = pos + len == payload.len();
                            rx.decompress_fragment(
                                &payload[pos..pos + len],
                                last,
                                &mut out,
                                1 << 20,
                            )
                            .unwrap();
                            pos += len;
                            if last {
                                break;
                            }
                        }
                        assert_eq!(out, msg);
                    }
                }
            }
        }
    }
}

/// ws-net: a message over the budget fails during inflation with
/// OutputLimit (close 1009); the connection's next message is never decoded
/// with a corrupt window, because the decompressor resets.
#[test]
fn test_consumer_ws_net_budget_and_close_codes() {
    let config = MessageConfig::new().with_context_takeover(true);
    let mut tx = Compressor::new(config).unwrap();
    let mut rx = Decompressor::new(config).unwrap();
    let mut payload = Vec::new();
    tx.compress_message(&vec![b'x'; 100_000], &mut payload)
        .unwrap();
    let err = rx
        .decompress_message(&payload, &mut Vec::new(), 99_999)
        .unwrap_err();
    assert_eq!(err.kind(), ErrorKind::OutputLimit); // 1009
    // Undecodable data is any other kind (1007).
    let err = rx
        .decompress_message(&[0xFF, 0xFF, 0xFF], &mut Vec::new(), 1 << 20)
        .unwrap_err();
    assert_ne!(err.kind(), ErrorKind::OutputLimit);
}

/// ws-net PreparedMessage: compressed once without context, then sent to
/// many connections, including ones whose receivers keep a window.
#[test]
fn test_consumer_ws_net_prepared_message_broadcast() {
    let mut x = 7;
    let broadcast = json_message(&mut x);
    let mut prepared = Vec::new();
    Compressor::new(MessageConfig::new())
        .unwrap()
        .compress_message(&broadcast, &mut prepared)
        .unwrap();
    for takeover in [false, true] {
        for bits in [15u8, 12] {
            let config = MessageConfig::new()
                .with_window_bits(bits)
                .with_context_takeover(takeover);
            // The connection's own messages first, then the shared one.
            let mut tx = Compressor::new(config).unwrap();
            let mut rx = Decompressor::new(config).unwrap();
            for _ in 0..3 {
                let own = json_message(&mut x);
                let mut p = Vec::new();
                tx.compress_message(&own, &mut p).unwrap();
                let mut out = Vec::new();
                rx.decompress_message(&p, &mut out, 1 << 20).unwrap();
                assert_eq!(out, own);
            }
            let mut out = Vec::new();
            rx.decompress_message(&prepared, &mut out, 1 << 20).unwrap();
            assert_eq!(out, broadcast);
        }
    }
}

/// http-net: a gzip request body arriving in chunks of any size, decoded as
/// it arrives with a cap; a body with two codings (`deflate, gzip`) decoded
/// in reverse order by two chained decoders.
#[test]
fn test_consumer_http_net_streaming_bodies() {
    let mut x = 99;
    let body: Vec<u8> = (0..300).flat_map(|_| json_message(&mut x)).collect();
    let limits = Limits::new().with_max_output(16 << 20).with_max_ratio(200);

    let mut wire = Vec::new();
    Encoder::new(Format::Gzip, Level::DEFAULT)
        .compress_vec(&body, &mut wire, Flush::Finish)
        .unwrap();
    for chunk in [1usize, 17, 4096, 65_536] {
        let mut dec = Decoder::new(Format::Gzip).with_limits(limits);
        let mut out = Vec::new();
        let pieces: Vec<&[u8]> = wire.chunks(chunk).collect();
        for (i, piece) in pieces.iter().enumerate() {
            if i + 1 == pieces.len() {
                dec.finish_vec(piece, &mut out).unwrap();
            } else {
                dec.decompress_vec(piece, &mut out).unwrap();
            }
        }
        assert_eq!(out, body, "chunks of {chunk}");
    }

    // Content-Encoding: deflate, gzip (deflate applied first).
    let mut inner = Vec::new();
    Encoder::new(Format::Zlib, Level::FAST)
        .compress_vec(&body, &mut inner, Flush::Finish)
        .unwrap();
    let mut outer = Vec::new();
    Encoder::new(Format::Gzip, Level::FAST)
        .compress_vec(&inner, &mut outer, Flush::Finish)
        .unwrap();
    let mut gz = Decoder::new(Format::Gzip).with_limits(limits);
    let mut z = Decoder::new(Format::Zlib).with_limits(limits);
    let mut middle = [0u8; 1000];
    let mut out = Vec::new();
    let mut input = &outer[..];
    loop {
        let p = gz.decompress(input, &mut middle).unwrap();
        input = &input[p.consumed..];
        z.decompress_vec(&middle[..p.produced], &mut out).unwrap();
        if input.is_empty() && p.status != Status::NeedsOutput {
            break;
        }
    }
    gz.finish(&[], &mut middle).unwrap();
    z.finish_vec(&[], &mut out).unwrap();
    assert_eq!(out, body);
}

/// serve-net: a streamed response (server-sent events), sync-flushed after
/// every event so the client can show each one at once.
#[test]
fn test_consumer_serve_net_streamed_response() {
    let mut enc = Encoder::new(Format::Gzip, Level::DEFAULT);
    let mut dec = Decoder::new(Format::Gzip);
    let mut shown = Vec::new();
    let mut sent = Vec::new();
    let mut x = 5;
    for i in 0..50 {
        let event = format!(
            "event: tick\ndata: {{\"n\":{i},\"v\":{}}}\n\n",
            next(&mut x) % 1000
        );
        sent.extend_from_slice(event.as_bytes());
        let mut wire = Vec::new();
        let flush = if i == 49 { Flush::Finish } else { Flush::Sync };
        enc.compress_vec(event.as_bytes(), &mut wire, flush)
            .unwrap();
        // Everything sent so far decodes from what is on the wire so far.
        if i == 49 {
            dec.finish_vec(&wire, &mut shown).unwrap();
        } else {
            let p = dec.decompress_vec(&wire, &mut shown).unwrap();
            assert_eq!(p.status, Status::NeedsInput);
        }
        assert_eq!(shown, sent, "event {i}");
    }
    assert!(dec.is_done());
}

/// serve-net direct mode: one encoder per response, reset and reused.
#[test]
fn test_consumer_serve_net_encoder_reuse() {
    let mut enc = Encoder::new(Format::Gzip, Level::DEFAULT);
    let mut x = 3;
    for _ in 0..20 {
        let body: Vec<u8> = (0..20).flat_map(|_| json_message(&mut x)).collect();
        let mut wire = Vec::new();
        enc.compress_vec(&body, &mut wire, Flush::Finish).unwrap();
        assert_eq!(compress_lib::gzip::decompress(&wire).unwrap(), body);
        enc.reset();
    }
}

/// Every engine can move to another thread and be shared (NFR-9): checked
/// at compile time.
#[test]
fn test_consumer_engines_are_send_and_sync() {
    fn check<T: Send + Sync>() {}
    check::<compress_lib::deflate::Inflater>();
    check::<compress_lib::deflate::Deflater>();
    check::<compress_lib::zlib::Decoder>();
    check::<compress_lib::zlib::Encoder>();
    check::<compress_lib::gzip::Decoder>();
    check::<compress_lib::gzip::Encoder>();
    check::<Decoder>();
    check::<Encoder>();
    check::<Compressor>();
    check::<Decompressor>();
    #[cfg(feature = "std")]
    {
        check::<compress_lib::io::DecodeReader<&[u8]>>();
        check::<compress_lib::io::EncodeWriter<Vec<u8>>>();
    }
    check::<compress_lib::Error>();
    check::<Limits>();
}
