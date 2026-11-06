<h1 align="center">
    <img width="90px" height="auto" src="https://raw.githubusercontent.com/jamesgober/jamesgober/main/media/icons/hexagon-3.svg" alt="Triple Hexagon">
    <br><b>CHANGELOG</b>
</h1>
<p>
  All notable changes to <code>compress-lib</code> will be documented in this file. The format is based on <a href="https://keepachangelog.com/en/1.1.0/">Keep a Changelog</a>,
  and this project adheres to <a href="https://semver.org/spec/v2.0.0.html/">Semantic Versioning</a>.
</p>

---

## [Unreleased]

---

## [0.5.0] - 2026-11-06

zlib and gzip framing, format detection, and the `std::io` adapters:
feature complete for 1.0.

### Added

- `zlib`: `compress`, `compress_with`, `decompress`, `decompress_with`,
  streaming `Decoder` (header checks, window limit, preset dictionaries by
  id, Adler-32 verified) and `Encoder` (FLEVEL, FDICT, window bits 8 to
  15).
- `gzip`: `compress`, `compress_with`, `decompress`, `decompress_with`,
  streaming `Decoder` (every header flag, FHCRC verified, CRC-32 and ISIZE
  verified, multi-member with `max_members`, output cap across members,
  header fields bounded by `max_header` and kept only on request, trailing
  data refused unless allowed) and `Encoder` with `GzipHeader` (name,
  comment, extra field, time stamp, OS; defaults that give the same bytes on
  every platform).
- `Format` with `detect`, the format-generic `Decoder` and `Encoder`, and
  `compress_lib::decompress` / `decompress_with` (gzip or zlib, detected).
- `io` (feature `std`): `Reader`, `Writer`, `Encoder`, `DecodeWriter`.
- Interoperation tests with zlib (through zlib-rs) both ways for zlib and
  gzip; CPython vectors; the `framing` fuzz target; the `gzip_file`,
  `websocket_message`, and `http_body` examples.

## [0.4.0] - 2026-11-06

The raw DEFLATE encoder and the WebSocket permessage-deflate helpers.

### Added

- `deflate::Deflater`: streaming raw DEFLATE (RFC 1951) encoding at levels
  0 to 9 (zlib's match-finder settings), window bits 8 to 15 (8 is a real
  256-byte window), `Flush::{None, Sync, Full, Finish}`, `compress`,
  `compress_vec`, `set_level` mid-stream, `set_dictionary`, `reset`,
  `total_in`, `total_out`, `memory_in_use`. Each block is written as
  stored, fixed, or dynamic Huffman, whichever is smallest. A round that
  would make no progress fails with `InvalidState` instead of looping.
- `deflate::compress` and `deflate::compress_with`.
- `message`: `MessageConfig`, `Compressor` (`compress_message`: sync flush,
  tail removed, context takeover on or off), `Decompressor`
  (`decompress_message`, `decompress_fragment`: tail appended, output
  budget checked while inflating, final blocks accepted, the vector as the
  message's history so no window is kept without takeover).
- Round-trip tests at every level, window, and flush pattern, checked by
  zlib (through zlib-rs) and miniz_oxide; RFC 7692 section 7.2.3 vectors;
  the `roundtrip` and `message` fuzz targets; the `deflate` benchmark.

## [0.3.0] - 2026-11-06

The raw DEFLATE inflater.

### Added

- `deflate::Inflater`: streaming raw DEFLATE (RFC 1951) decoding with
  caller-owned buffers (`decompress`, `finish`, `decompress_vec`,
  `finish_vec`), window bits 8 to 15 enforced exactly, `Limits` checked
  before every output chunk, sticky errors, `reset`, `next_stream` (a new
  stream keeping the window, for RFC 7692 takeover), `set_dictionary`,
  `total_in`, `total_out`, `memory_in_use`.
- `deflate::decompress` and `deflate::decompress_with`: one call, truncation
  and trailing data reported.
- `Progress` and `Status` for every streaming call.
- `ErrorKind::AllocationFailed`: vector growth uses `try_reserve`.
- Differential tests against zlib (through zlib-rs) and miniz_oxide,
  counting-allocator tests, the `inflate_raw` and `inflate_split` fuzz
  targets with a fuzz smoke job in CI, the `inflate` benchmark, and the
  `streaming` example.

## [0.2.0] - 2026-11-06

The design lock and the first code: checksums, errors, limits, and levels.

### Added

- `docs/API.md`: the complete planned 1.0 API with the version each item
  arrives in.
- `checksum`: `crc32` (slicing-by-16, tables built at compile time),
  `Crc32` (streaming, `from_value`, `combine`), `crc32_combine`; `adler32`
  (four lanes, deferred modulo), `Adler32`, `adler32_combine`.
- `Error` (kind and input offset, `code()`), `ErrorKind` (every kind the
  codecs through 0.5 return, each with a stable code and a message that says
  what to do), `From<Error> for std::io::Error`.
- `Limits` (64 MiB output cap by default, optional ratio cap with a floor,
  gzip header and member caps, `unlimited()`), `Level` (0 to 9).
- Checksum benchmarks (`benches/checksum.rs`).

## [0.1.0] - 2026-11-05

The scaffold: the repository, the plan, and CI. No codec yet.

### Added

- Crate manifest: edition 2024, MSRV 1.85, no dependencies, `Apache-2.0 OR MIT`,
  feature `std` (default; without it the crate is `no_std` with `alloc`).
- Crate root: `VERSION`, `unsafe_code` forbidden, missing docs denied, and
  `unwrap`/`expect`/`panic` denied in library code.
- CI on Linux, macOS, and Windows, each on x86_64 and aarch64, on stable and
  1.85; checks for i686, s390x (big-endian), wasm32, and a bare-metal `no_std`
  target; Miri on x86_64 and s390x; a job that fails on any runtime
  dependency; `cargo package`; `cargo audit` and `cargo deny`.
- `dev/PLAN.md` (purpose, scope, requirements FR-1 to FR-17 and NFR-1 to
  NFR-9, design, the API with item ids and versions, parity matrix, threats
  T-1 to T-14, performance targets, testing plan with edge cases E-1 to E-30,
  roadmap, open decisions, ideas, decision log D-001 to D-013),
  `dev/DIRECTIVES.md`, and `dev/ROADMAP.md`.
- `fuzz/` skeleton.

[Unreleased]: https://github.com/jamesgober/compress-lib/compare/v0.5.0...HEAD
[0.5.0]: https://github.com/jamesgober/compress-lib/compare/v0.4.0...v0.5.0
[0.4.0]: https://github.com/jamesgober/compress-lib/compare/v0.3.0...v0.4.0
[0.3.0]: https://github.com/jamesgober/compress-lib/compare/v0.2.0...v0.3.0
[0.2.0]: https://github.com/jamesgober/compress-lib/compare/v0.1.0...v0.2.0
[0.1.0]: https://github.com/jamesgober/compress-lib/releases/tag/v0.1.0
