# compress-lib - Roadmap

> Path from scaffold to a stable 1.0. Each phase has hard exit criteria and ends with the release routine (bump, CHANGELOG, release notes, local gate, push on a branch with a PR, green CI, squash-merge, tag, GitHub pre-release, `cargo doc`, `cargo publish`, verify crates.io and docs.rs).
> Design, API item ids (ROOT-, ERR-, LIM-, LVL-, CK-, DF-, MSG-, ZL-, GZ-, FMT-, IO-, BR-), threats (T-1 to T-14), and edge cases (E-1 to E-30): [`PLAN.md`](./PLAN.md).
>
> **Anti-deferral rule:** no listed task moves to a later phase unless this file records the move and the reason, with a decision-log entry in `PLAN.md`. P0 items ship in the version listed and never move.

## v0.1.0 - Scaffold (DONE, 2026-11-05)
Exit: CI green on every job; the crate published to claim the name; decision log D-001 to D-013 written.
- [x] Repository `jamesgober/compress-lib` created, `lib-collection/compress-lib` pushed.
- [x] Manifest (edition 2024, MSRV 1.85, feature `std`, no dependencies), README, CHANGELOG, REPS, dual license, `deny.toml`, `clippy.toml`, `rustfmt.toml`, `.editorconfig`, `.gitattributes`.
- [x] `src/lib.rs`: crate docs, lint attributes, `forbid(unsafe_code)`, `VERSION` (ROOT-01), one test.
- [x] `dev/PLAN.md`, `dev/DIRECTIVES.md`, `dev/ROADMAP.md`; `fuzz/` skeleton.
- [x] CI: Linux, macOS, Windows on x86_64 and aarch64, stable and 1.85; cross checks (i686, s390x, wasm32, thumbv7em); Miri on x86_64 and s390x; no-runtime-dependency job; package; audit and deny.

## v0.2.0 - Design lock (DONE, 2026-11-06)
API: ROOT-02 · ERR-16 (type and codes) · LIM-01 to LIM-04 · LVL-01 · CK-01 to CK-04.
Exit: `docs/API.md` lists the whole 1.0 API with the version of each item (D-010); checksums pass the published check values and match a reference on random data; checksum benchmarks recorded.
- [x] `docs/API.md` in the metrics-lib format, every item of `PLAN.md` section 6.
- [x] `Error`, `ErrorKind` (every kind through 0.5, plus `AllocationFailed`, D-014), `Limits`, `Level`.
- [x] CRC-32 (slicing-by-16) and Adler-32 (deferred modulo), one-shot, streaming, `combine`.
- [x] One new defence idea recorded.

## v0.3.0 - Core: inflate (DONE, 2026-11-06)
API: DF-01, DF-02, DF-10 to DF-19 (DF-17 to DF-19 brought forward from 0.4, D-024) · ERR-01 to ERR-10.
Exit: every inflate edge case (E-1, E-3 to E-19) has a test; differential tests against miniz_oxide agree on every accept, reject, and output; streaming split invariance at every byte; counting-allocator proof; fuzz smoke in CI.
- [x] Bit reader, table builder (fixed tables at compile time), state machine, fast loop, window ring.
- [x] Limits checked per chunk (T-1, T-2), window bits 8 to 15 (T-4), linear work with tiny buffers (T-5, T-6), truncation and trailing data (T-12, T-13).
- [x] Fuzz targets `inflate_raw`, `inflate_split`.

## v0.4.0 - Extended: deflate and RFC 7692 (DONE, 2026-11-06)
API: DF-03, DF-04, DF-30 to DF-36 · MSG-01 to MSG-07 · ERR-10 (level).
Exit: round trips at every level, window, and flush pattern; miniz_oxide decodes all output; RFC 7692 section 7.2.3 vectors; ws-net's permessage-deflate can be built on it.
- [x] Stored, greedy, and lazy strategies; dynamic, fixed, and stored block choice; flush modes; dictionaries; window bits 8 to 15.
- [x] `message::Compressor`, `message::Decompressor`, context takeover on and off, `BFINAL` handling.
- [x] Threats T-9, T-10, T-14; fuzz targets `roundtrip` and `message`.
- Moved to 0.6 (D-031): deflate speed against the section 9 targets (sizes already match zlib; speed at level 1 measured well behind miniz_oxide and zlib-rs on the shared machine).

## v0.5.0 - Feature complete (DONE, 2026-11-06)
API: ZL-01, ZL-02, GZ-01 to GZ-03, FMT-01 to FMT-03, IO-01 to IO-03, LIM-05 · ERR-11 to ERR-15.
Exit: every FR-1 to FR-16 met; files produced by GNU gzip and zlib decode; T-6 (members), T-7, T-8 tests pass.
- [x] zlib and gzip decoders and encoders, `Format`, `Decoder`, `Encoder`, auto-detect, `io` adapters.
- [x] Interoperation with zlib both ways; CPython gzip vectors; T-6 (members), T-7 (header cap), T-8 (checksums) tests (`tests/framing.rs`).
- [x] Fuzz target `framing`; examples `gzip_file`, `websocket_message`, `http_body`.

## v0.6.0 - Optimisation (DONE, 2026-11-06)
Exit: `bench-compare/` against miniz_oxide, zlib-rs, and flate2 backends on Linux, macOS, and Windows runners; `docs/PERFORMANCE.md` with measured numbers; `PLAN.md` section 9 targets met or the gap recorded with its reason; `MemoryConfig` (DF-37).
- [x] `bench-compare/` (miniz_oxide, zlib-rs, flate2 over C zlib and zlib-rs) and the hosted `Bench` workflow on Linux, macOS, and Windows.
- [x] `docs/PERFORMANCE.md` with measured numbers.
- [x] Match finders and block writer tuned (D-041, D-042); level 1 redefined (D-040); small-message paths (D-043, D-044); `MemoryConfig` (DF-37).
- [x] Gap recorded (D-045): compression at levels 6 and 9 is behind zlib-rs, which uses SIMD and `unsafe`; levels 1 and 6 are at or ahead of miniz_oxide; decoding leads miniz_oxide on most inputs.

## v0.7.0 - Hardening (DONE, 2026-11-06)
Exit: every fuzz target complete; 24 h fuzz run of the inflater targets without a finding; attack test per threat (`tests/attacks.rs`); 4 h soak flat; `docs/SECURITY.md`.
- [x] Fuzz targets complete: `inflate_raw`, `inflate_split`, `roundtrip`, `message`, `framing`; smoke on every push, ten minutes per target nightly.
- [x] 24 h fuzz run of the inflater targets (moved to before 0.9, D-050): restarted on the fixed code (D-054); done in 0.9 (D-053).
- [x] Attack test per threat (`tests/attacks.rs`).
- [x] 4 h soak on the fixed code: 446,443 random round trips through every format, level, window, and flush pattern, with the live heap flat at 21,318 bytes and a peak of 1,731,540 bytes.
- [x] `docs/SECURITY.md`.

## v0.8.0 - Alpha (DONE, 2026-11-06)
Exit: ws-net permessage-deflate on a branch passes Autobahn sections 12 and 13 strict; http-net body decode and serve-net response encode examples run; consumer issues fixed.
- [x] The consumer paths, called as ws-net, http-net, and serve-net call them (`tests/consumers.rs`); the two issues ws-net's review of 0.6.0 found (quadratic CPU from byte-by-byte fragments, a cut-short final block accepted) are fixed in 0.7.0 (D-054), with attack tests T-5 and T-12.
- [x] Interoperation with C zlib both ways in CI (CPython's `zlib` and `gzip`).
- Moved out (D-050): permessage-deflate inside ws-net with Autobahn sections 12 and 13 is ws-net's own milestone, built in its repository against the published `message` API; this crate's side is the RFC 7692 codec and its tests here.

## v0.9.x - Beta and polish (0.9.0 DONE, 2026-11-08)
Exit: API audit (names, `#[must_use]`, `#[non_exhaustive]`, Tier 1 first), docs audit (every claim matches the build), `docs/ARCHITECTURE.md`, `docs/PLATFORM-NOTES.md`, `docs/STABILITY-1.0.md` drafted, release candidates; nothing open except the 1.0 decision.
- [x] API audit: `io::Encoder` renamed `io::EncodeReader` (D-051); `#[must_use]` on the `io` constructors; every public item documented with a runnable example.
- [x] Docs audit: README, `docs/API.md`, and rustdoc checked against the build; `docs/ARCHITECTURE.md`, `docs/PLATFORM-NOTES.md`, `docs/STABILITY-1.0.md`.
- [x] 24 h inflater fuzz run (D-050): 24 hours of fuzzing per decoder target (`inflate_raw`, `inflate_split`) on the 0.9 decoder, 78,459,234 and 21,727,907 runs, with no findings.
- Release candidates (`1.0.0-rc.N`) were not needed: the 1.0 polish pass ran on the main line with a fresh 24 h fuzz run and soak on the final code (D-061).

## v1.0.0 - Stable (DONE, 2026-11-10)
Exit: James's go; the audit recorded in `docs/STABILITY-1.0.md`; every gap, deferral, and missed target revisited; a fresh 24 h fuzz run and 4 h soak on the final code.
- [x] Polish pass: decoder vector paths and short periods (D-058); level 1, the block writer, and chain walks (D-055, D-056, D-058); measured on hosted runners and recorded (D-061).
- [x] Audit fixes: output independent of input splits at levels 0 and 1 (D-057); one-shot compression never returns a truncated stream (D-059); the 1.0 soak found an index underflow in the new short-period decoder code, fixed before release, and the `inflate_split` target widened to reach it (D-062).
- [x] API review for the 1.x freeze: `io::DecodeReader`, `io::EncodeWriter`, `Flush` at the root (D-060); cargo-semver-checks against 0.9.0.
- [x] 24 h fuzz on the final code (`inflate_raw` 151,357,523 runs, `inflate_split` 101,726,569, `roundtrip` 56,085,017; 309 million in all), no findings.
- [x] 4 h soak on the final code: 2,098,028 rounds in 4 h 11 min of wall time, heap flat (21,318 bytes live between rounds, peak 1,745,428).

## v1.1.0 - Review fixes (2026-11-12)
Exit: every finding of the external review of 1.0 fixed or documented, each with a regression test that fails on 1.0; the fuzz targets and the soak run on the fixed code; hosted benchmarks re-run.
- [x] F-1: the ratio cap counts every gzip member together (D-064).
- [x] F-2: four-byte hash at levels 4 to 9 and a per-byte chain budget (D-065).
- [x] F-3: a deflater's memory stays at its built size (D-064).
- [x] F-4: `io::DecodeReader` refuses trailing data; `with_trailing_data` (D-064).
- [x] F-5: `DictionaryRequired` is resumable (D-064).
- [x] F-6 to F-8: doc figures, `wipe` and redacted `Debug` on the framing types, the gzip trailing-data rule (D-064); `gzip::Decoder::reset` fixed.
- [ ] Fuzz and soak on the fixed code; hosted benchmarks; release.

## After 1.1
- 1.2: brotli decode (BR-01), feature `brotli` (D-066).
- 1.3: brotli encode (BR-02).
