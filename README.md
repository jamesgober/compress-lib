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
        <strong>Status: <code>0.8.0</code>, alpha.</strong> Feature complete, measured (<a href="./docs/PERFORMANCE.md"><code>docs/PERFORMANCE.md</code></a>), hardened (<a href="./docs/SECURITY.md"><code>docs/SECURITY.md</code></a>), and exercised the way WebSocket, HTTP, and file servers use it, with every stream checked against C zlib in both directions. The beta series (<code>0.9</code>) and the 1.0 audit come next; the complete planned API is in <a href="./docs/API.md"><code>docs/API.md</code></a>. Design: <a href="./dev/PLAN.md"><code>dev/PLAN.md</code></a>; schedule: <a href="./dev/ROADMAP.md"><code>dev/ROADMAP.md</code></a>.
    </blockquote>
</div>

<hr>
<br>

## Scope

Raw DEFLATE inflate and deflate with window bits 8 to 15 and levels 0 to 9; sync, full, and finish flushes; zlib and gzip framing with checksums always verified; preset dictionaries; per-message compression for WebSocket servers and clients (context takeover on or off); CRC-32 and Adler-32; `std::io` adapters. Brotli follows after 1.0. It does not negotiate HTTP or WebSocket headers (http-net and ws-net do) and it does not read archives.

## Quick start

```toml
[dependencies]
compress = { package = "compress-lib", version = "0.8" }
```

One call each way, capped at 64 MiB of output by default:

```rust
let packed = compress::gzip::compress(body);          // Vec<u8>, level 6
let body = compress::gzip::decompress(&packed)?;      // all members, checksums verified
let z = compress::zlib::compress(data);               // HTTP's "deflate" coding
let raw = compress::deflate::compress(data);          // raw DEFLATE
let any = compress::decompress(&packed)?;             // gzip or zlib, detected
```

Limits for untrusted input:

```rust
let limits = compress::Limits::new().with_max_output(8 << 20).with_max_ratio(50);
let body = compress::gzip::decompress_with(&wire, limits)?;   // OutputLimit, RatioLimit
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

Streaming, with buffers you own, for any format:

```rust
use compress::{Decoder, Format, deflate::Status};

let mut decoder = Decoder::new(Format::Gzip);
let p = decoder.decompress(chunk, &mut output)?;       // push input, pull output
match p.status {
    Status::NeedsInput => { /* feed chunk[p.consumed..] and more; finish() with the last */ }
    Status::NeedsOutput => { /* drain output[..p.produced], call again */ }
    _ => { /* Status::Done */ }
}
```

And `std::io`: `compress::io::Reader`, `Writer`, `EncodeReader`, `DecodeWriter`.

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
