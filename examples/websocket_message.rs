//! The codec side of WebSocket permessage-deflate (RFC 7692): what a server
//! and a client each hold, and what goes on the wire.

use compress_lib::{
    ErrorKind,
    message::{Compressor, Decompressor, MessageConfig},
};

fn main() -> Result<(), compress_lib::Error> {
    // Negotiated: server_no_context_takeover, client_max_window_bits=12.
    let server_to_client = MessageConfig::new();
    let client_to_server = MessageConfig::new()
        .with_window_bits(12)
        .with_context_takeover(true);

    let mut server_tx = Compressor::new(server_to_client)?;
    let mut client_rx = Decompressor::new(server_to_client)?;
    let mut client_tx = Compressor::new(client_to_server)?;
    let mut server_rx = Decompressor::new(client_to_server)?;

    for text in [
        &br#"{"op":"subscribe","topic":"prices"}"#[..],
        br#"{"op":"subscribe","topic":"trades"}"#,
    ] {
        let mut payload = Vec::new();
        client_tx.compress_message(text, &mut payload)?;
        let mut message = Vec::new();
        server_rx.decompress_message(&payload, &mut message, 1 << 20)?;
        println!(
            "client -> server: {} bytes on the wire for {} bytes",
            payload.len(),
            message.len()
        );
    }

    let mut payload = Vec::new();
    server_tx.compress_message(br#"{"price":101.25}"#, &mut payload)?;
    let mut message = Vec::new();
    client_rx.decompress_message(&payload, &mut message, 1 << 20)?;
    println!("server -> client: {}", String::from_utf8_lossy(&message));

    // A compressed bomb stops at the message budget: close with 1009.
    let mut bomb = Vec::new();
    server_tx.compress_message(&vec![0u8; 16 << 20], &mut bomb)?;
    let err = client_rx
        .decompress_message(&bomb, &mut Vec::new(), 1 << 20)
        .unwrap_err();
    assert_eq!(err.kind(), ErrorKind::OutputLimit);
    println!(
        "{} wire bytes refused at the 1 MiB budget: {err}",
        bomb.len()
    );
    Ok(())
}
