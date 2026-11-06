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

## v0.6.0 - Optimisation
Exit: `bench-compare/` against miniz_oxide, zlib-rs, and flate2 backends on Linux, macOS, and Windows runners; `docs/PERFORMANCE.md` with measured numbers; `PLAN.md` section 9 targets met or the gap recorded with its reason; `MemoryConfig` (DF-37).

## v0.7.0 - Hardening
Exit: every fuzz target complete; 24 h fuzz run of the inflater targets without a finding; attack test per threat (`tests/attacks.rs`); 4 h soak flat; `docs/SECURITY.md`.

## v0.8.0 - Alpha
Exit: ws-net permessage-deflate on a branch passes Autobahn sections 12 and 13 strict; http-net body decode and serve-net response encode examples run; consumer issues fixed.

## v0.9.x - Beta and polish
Exit: API audit (names, `#[must_use]`, `#[non_exhaustive]`, Tier 1 first), docs audit (every claim matches the build), `docs/ARCHITECTURE.md`, `docs/PLATFORM-NOTES.md`, `docs/STABILITY-1.0.md` drafted, release candidates; nothing open except the 1.0 decision.

## v1.0.0 - Stable
Only on James's explicit go, after the audit is recorded in `docs/STABILITY-1.0.md`.

## After 1.0
- 1.1: brotli decode (BR-01), feature `brotli`.
- 1.2: brotli encode (BR-02).
