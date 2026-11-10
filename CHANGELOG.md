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

## [1.0.0] - 2026-11-10

Stable: the API frozen for 1.x, faster on both sides, and three defects
found by the 1.0 audit fixed.

### Changed

- `io::Reader` is now `io::DecodeReader` and `io::Writer` is now
  `io::EncodeWriter`, so all four adapters say what they do
  (`DecodeReader`, `EncodeWriter`, `EncodeReader`, `DecodeWriter`). This is
  the only breaking change from 0.9.
- `Flush` is also exported at the crate root, beside `Progress` and
  `Status`.
- Level 1 writes a three-byte match more than 8 KiB back as literals:
  output is 2.3 to 2.5 percent smaller on JSON and HTML. The compressed
  bytes of levels 2 to 9 are unchanged.
- Faster: decoding through the vector and one-shot paths (1.06 to 1.28
  times 0.9 on Windows), periods of 2 to 7 bytes in the decoder's fast
  loop, and compression at every level (up to 1.31 times 0.9), from a dedicated
  level-1 loop, a block writer with per-block code tables and one store
  per symbol, and a wider check before each chain candidate.

### Fixed

- The compressed bytes at levels 0 and 1 depended on how the input was
  split across calls (level 0 cut stored blocks where input arrived; level
  1 restarted its run of misses on every call). Output now depends only on
  the input, the settings, and the flush sequence, at every level.
- The one-shot `compress` functions (raw DEFLATE, zlib, gzip) returned a
  truncated stream as if complete when the output vector could not grow.
  They now abort, as growing a `Vec` does; the streaming encoders still
  report `AllocationFailed`.

### Verified

- 24-hour fuzz run on the final code (`inflate_raw` 151,357,523 runs, `inflate_split` 101,726,569, `roundtrip` 56,085,017; 309 million in all), no findings.
- 4-hour soak on the final code: 2,098,028 rounds in 4 h 11 min of wall time, heap flat (21,318 bytes live between rounds, peak 1,745,428).
- cargo-semver-checks against 0.9.0: the two `io` renames are the only
  breaking changes.

## [0.9.0] - 2026-11-08

The beta: the 1.0 API audit and the documents that go with 1.0.

### Changed

- `io::Encoder` is now `io::EncodeReader`, the pair of `io::DecodeWriter`
  (it read like the format-generic `Encoder`).
- The `io` constructors and `Reader::into_inner` are `#[must_use]`.

### Added

- `docs/ARCHITECTURE.md`, `docs/PLATFORM-NOTES.md`, and
  `docs/STABILITY-1.0.md` (what 1.0 promises and the audit, item by item).
- An example on `Flush`; the edge cases E-1 to E-30 mapped to their tests
  in `dev/PLAN.md`.

### Verified

- 24 hours of fuzzing per decoder target (`inflate_raw`, `inflate_split`) on the 0.9 decoder, 78,459,234 and 21,727,907 runs, with no findings.

## [0.8.0] - 2026-11-06

The alpha: the consumer paths end to end, and C zlib on both sides.

### Added

- `tests/consumers.rs`: WebSocket permessage-deflate under every
  negotiated window size and takeover setting with messages split into
  frames at random points, the per-message budget and the close codes it
  maps to, prepared (broadcast) messages; HTTP bodies decoded in chunks of
  any size and with two codings (`deflate, gzip`) by chained decoders;
  sync-flushed streamed responses decodable after every event; encoder
  reuse; a compile-time check that every engine is `Send` and `Sync`.
- Interoperation with C zlib through CPython's `zlib` and `gzip` modules,
  both directions, every format, levels 0, 1, 6, and 9, multi-member gzip
  and sync points: `tests/interop/check.py`, the `interop` example, and a
  CI job.

## [0.7.0] - 2026-11-06

Hardening: attack tests, a soak, the threat model, and two fixes.

### Added

- `tests/attacks.rs`: one attack test per threat T-1 to T-14 of the plan
  (bombs in every format, ratio bombs, malformed tables, distances beyond
  the data or window, tiny buffers, empty blocks and members, gzip header
  floods, corrupted checksums, cross-message compression oracles, memory per
  engine, arbitrary input, truncation, trailing data, adversarial encoder
  input).
- `tests/soak.rs` (ignored by default): random round trips through every
  format, level, window, and flush pattern with a heap check.
- `docs/SECURITY.md`: the threat model, each defence with its cost and its
  test, and guidance on limits and compression side channels.
- A nightly fuzz workflow (ten minutes per target).
- `wipe()` on `Inflater`, `Deflater`, `message::Compressor`, and
  `message::Decompressor`: zeroes the history, tables, and buffers (spare
  capacity included) for connections that carried secrets (threat T-15).

### Fixed

- Vector decoding (`decompress_vec`, `finish_vec`, the zlib and gzip
  decoders, and `message::Decompressor`) zero-filled everything the stream
  had produced so far on every call, so a WebSocket message fed one
  compressed byte per call cost quadratic CPU (a 16 MiB message took about
  15 seconds instead of 13 ms). Each call now costs `O(input + output)`
  (threat T-5). The encoders' vector paths zero-filled all spare capacity
  on every call; they now fill only the step's room.
- `message::Decompressor` accepted a message whose last block was cut short
  (a stored block missing bytes, a Huffman block without its end code); with
  context takeover the next message then decoded as garbage. A message must
  now end on a block boundary once its `00 00 FF FF` tail is appended, or
  it is `UnexpectedEnd` (threat T-12, RFC 7692 section 7.2.2). A sender
  that leaves the tail in place, which RFC 7692 forbids, is refused too.

### Verified

- A 24-hour fuzz run of the decoder targets on the fixed code is clean so far and finishes before 0.9.
- A 4-hour soak on the fixed code: 446,443 random round trips through every format, level, window, and flush pattern, with the live heap flat at 21,318 bytes and a peak of 1,731,540 bytes.

## [0.6.0] - 2026-11-06

Optimisation, measured against miniz_oxide, zlib-rs, and flate2.

### Added

- `deflate::MemoryConfig` (`with_hash_bits`, `with_block_symbols`) and
  `Deflater::with_memory`; `MessageConfig::with_memory`.
- `bench-compare/`: benchmarks against miniz_oxide, zlib-rs, and flate2
  over each backend, with a hosted-runner workflow (`Bench`).
- `docs/PERFORMANCE.md` with the measured numbers and the method.

### Changed

- The match finders keep their state in locals and compare four bytes at a
  time; the block writer writes each symbol's code and extra bits at once.
  The match decisions at levels 2 to 9 are the same as in 0.5.
- Level 1 probes one candidate in a 4,096-entry hash table and searches
  more sparsely through long runs without matches (incompressible input):
  faster, and its output is still smaller than miniz_oxide's and zlib-rs's
  level 1.
- `Deflater::reset` clears only the hash entries a short stream used, so
  per-message compression (no context takeover) no longer clears 64 KiB
  each time.
- Vector decoding starts with room for four times the input (at most
  64 KiB) instead of 64 KiB, so small messages decode without a large
  allocation.
- Encoder positions start at 1, so a match can refer to the first byte of
  the stream (the second "Hello" of RFC 7692 section 7.2.3.2 is now one
  match, a byte shorter than zlib's encoding).

### Fixed

- The counting-allocator test counts only its own thread's allocations;
  the test harness allocating on another thread could fail it.

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

[Unreleased]: https://github.com/jamesgober/compress-lib/compare/v1.0.0...HEAD
[1.0.0]: https://github.com/jamesgober/compress-lib/compare/v0.9.0...v1.0.0
[0.9.0]: https://github.com/jamesgober/compress-lib/compare/v0.8.0...v0.9.0
[0.8.0]: https://github.com/jamesgober/compress-lib/compare/v0.7.0...v0.8.0
[0.7.0]: https://github.com/jamesgober/compress-lib/compare/v0.6.0...v0.7.0
[0.6.0]: https://github.com/jamesgober/compress-lib/compare/v0.5.0...v0.6.0
[0.5.0]: https://github.com/jamesgober/compress-lib/compare/v0.4.0...v0.5.0
[0.4.0]: https://github.com/jamesgober/compress-lib/compare/v0.3.0...v0.4.0
[0.3.0]: https://github.com/jamesgober/compress-lib/compare/v0.2.0...v0.3.0
[0.2.0]: https://github.com/jamesgober/compress-lib/compare/v0.1.0...v0.2.0
[0.1.0]: https://github.com/jamesgober/compress-lib/releases/tag/v0.1.0
