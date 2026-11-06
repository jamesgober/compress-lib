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

[Unreleased]: https://github.com/jamesgober/compress-lib/compare/v0.2.0...HEAD
[0.2.0]: https://github.com/jamesgober/compress-lib/compare/v0.1.0...v0.2.0
[0.1.0]: https://github.com/jamesgober/compress-lib/releases/tag/v0.1.0
