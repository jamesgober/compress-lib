//! Content codings for an HTTP layer: pick the format from
//! `Content-Encoding`, decode a body with a cap, encode a response.

use compress_lib::{Decoder, Encoder, Format, Level, Limits, deflate::Flush};

/// The format for one `Content-Encoding` token (RFC 9110 section 8.4.1).
fn format_for(coding: &str) -> Option<Format> {
    match coding.trim().to_ascii_lowercase().as_str() {
        "gzip" | "x-gzip" => Some(Format::Gzip),
        // HTTP's "deflate" is zlib framing, not raw DEFLATE.
        "deflate" => Some(Format::Zlib),
        _ => None,
    }
}

fn main() -> Result<(), compress_lib::Error> {
    // A response body, encoded once.
    let body = b"<html><body>".repeat(200);
    let mut encoder = Encoder::new(Format::Gzip, Level::DEFAULT);
    let mut wire = Vec::new();
    encoder.compress_vec(&body, &mut wire, Flush::Finish)?;
    println!(
        "Content-Encoding: gzip, {} bytes for {}",
        wire.len(),
        body.len()
    );

    // A request body arriving in chunks, capped at 1 MiB decoded.
    let format = format_for("gzip").unwrap_or(Format::Raw);
    let mut decoder = Decoder::new(format)
        .with_limits(Limits::new().with_max_output(1 << 20).with_max_ratio(100));
    let mut decoded = Vec::new();
    let chunks: Vec<&[u8]> = wire.chunks(512).collect();
    for (i, chunk) in chunks.iter().enumerate() {
        if i + 1 == chunks.len() {
            decoder.finish_vec(chunk, &mut decoded)?;
        } else {
            decoder.decompress_vec(chunk, &mut decoded)?;
        }
    }
    assert_eq!(decoded, body);
    println!(
        "decoded {} bytes from {} chunks",
        decoded.len(),
        chunks.len()
    );
    Ok(())
}
