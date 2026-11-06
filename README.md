<h1 align="center">
    <img width="99" alt="Rust logo" src="https://raw.githubusercontent.com/jamesgober/rust-collection/72baabd71f00e14aa9184efcb16fa3deddda3a0a/assets/rust-logo.svg">
    <br>
    <b>compress-lib</b>
    <br>
    <sub><sup>DEFLATE, ZLIB, GZIP</sup></sub>
</h1>

<div align="center">
    <a href="https://crates.io/crates/compress-lib"><img alt="Crates.io" src="https://img.shields.io/crates/v/compress-lib"></a>
    <a href="https://crates.io/crates/compress-lib"><img alt="Downloads" src="https://img.shields.io/crates/d/compress-lib?color=%230099ff"></a>
    <a href="https://docs.rs/compress-lib"><img alt="docs.rs" src="https://img.shields.io/docsrs/compress-lib"></a>
    <a href="https://github.com/jamesgober/compress-lib/actions"><img alt="CI" src="https://github.com/jamesgober/compress-lib/actions/workflows/ci.yml/badge.svg"></a>
    <a href="https://github.com/rust-lang/rfcs/blob/master/text/2495-min-rust-version.md"><img alt="MSRV" src="https://img.shields.io/badge/MSRV-1.85%2B-blue"></a>
</div>

<br>

<div align="left">
    <p>
        <strong>compress-lib</strong> compresses and decompresses the formats the web runs on: raw DEFLATE (RFC 1951), zlib (RFC 1950), and gzip (RFC 1952), with the per-message rules of WebSocket permessage-deflate (RFC 7692). One plain call covers the common case; a streaming engine underneath covers everything else.
    </p>
    <p>
        The engine is sans-IO: you push input and pull output through buffers you own, and it never touches a socket or a file. Memory is fixed when an engine is built and never grows with input. Every decoder enforces an absolute output cap, and optionally a ratio cap, while it decodes, so a decompression bomb costs at most the cap. There is no <code>unsafe</code> code and no third-party runtime dependency.
    </p>
    <br>
    <hr>
    <p>
        <strong>MSRV is 1.85+</strong> (Rust 2024 edition). <code>unsafe</code> is forbidden. <code>no_std</code> with <code>alloc</code> when the default <code>std</code> feature is off.
    </p>
    <blockquote>
        <strong>Status: <code>0.4.0</code>, deflate and RFC 7692.</strong> Raw DEFLATE both ways: the inflater (streaming, window bits 8 to 15, output and ratio caps checked before every output chunk, zlib's exact acceptance rules) and the deflater (levels 0 to 9, window bits 8 to 15, sync, full, and finish flushes, preset dictionaries), plus the <code>message</code> module for WebSocket permessage-deflate. zlib and gzip framing arrive in <code>0.5</code>; the complete planned API is in <a href="./docs/API.md"><code>docs/API.md</code></a>. Design: <a href="./dev/PLAN.md"><code>dev/PLAN.md</code></a>; schedule: <a href="./dev/ROADMAP.md"><code>dev/ROADMAP.md</code></a>.
    </blockquote>
</div>

<hr>
<br>

## Scope

Raw DEFLATE inflate and deflate with window bits 8 to 15 and levels 0 to 9; sync, full, and finish flushes; zlib and gzip framing with checksums always verified; preset dictionaries; per-message compression for WebSocket servers and clients (context takeover on or off); CRC-32 and Adler-32; `std::io` adapters. Brotli follows after 1.0. It does not negotiate HTTP or WebSocket headers (http-net and ws-net do) and it does not read archives.

## Quick start

```toml
[dependencies]
compress = { package = "compress-lib", version = "0.4" }
```

Available now:

```rust
let packed = compress::deflate::compress(data);        // raw DEFLATE, level 6
let data = compress::deflate::decompress(&packed)?;   // capped at 64 MiB
let crc = compress::checksum::crc32(&data);           // gzip's checksum
```

WebSocket permessage-deflate (RFC 7692), one direction each:

```rust
use compress::message::{Compressor, Decompressor, MessageConfig};

let config = MessageConfig::new().with_window_bits(12);   // no context takeover by default
let mut tx = Compressor::new(config)?;
let mut rx = Decompressor::new(config)?;
tx.compress_message(text, &mut payload)?;               // sync flush, tail removed
rx.decompress_message(&payload, &mut message, max)?;    // budget checked while inflating
```

Streaming, with buffers you own:

```rust
use compress::deflate::{Deflater, Flush, Inflater, Status};

let p = deflater.compress(input, &mut output, Flush::None)?;  // push input, pull output
let q = inflater.decompress(input, &mut output)?;
match q.status {
    Status::NeedsInput => { /* feed input[q.consumed..] and more */ }
    Status::NeedsOutput => { /* drain output[..q.produced], call again */ }
    _ => { /* Status::Done: the stream ended at input[q.consumed] */ }
}
```

Planned for 0.5, the same shape:

```rust
let packed = compress::gzip::compress(body);          // Vec<u8>, level 6
let body = compress::gzip::decompress(&packed)?;      // capped at 64 MiB by default
```

The complete planned API, with the version each item arrives in, is in [`docs/API.md`](./docs/API.md).

<hr>
<br>

## Contributing

See <a href="./dev/DIRECTIVES.md"><code>dev/DIRECTIVES.md</code></a> for engineering standards and the definition of done. Before a PR: `cargo fmt --all`, `cargo clippy --all-targets --all-features -- -D warnings`, and `cargo test --all-features` must be clean.

<br>

<div id="license">
    <h2>License</h2>
    <p>Licensed under either of</p>
    <ul>
        <li><b>Apache License, Version 2.0</b>: <a href="./LICENSE-APACHE">LICENSE-APACHE</a></li>
        <li><b>MIT License</b>: <a href="./LICENSE-MIT">LICENSE-MIT</a></li>
    </ul>
    <p>at your option.</p>
</div>

<div align="center">
  <h2></h2>
  <sup>COPYRIGHT <small>&copy;</small> 2026 <strong>James Gober.</strong></sup>
</div>
