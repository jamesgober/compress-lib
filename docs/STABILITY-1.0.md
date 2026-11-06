# Stability and the 1.0 audit

What 1.0 will promise, and the audit that must pass before it ships. 1.0 is released only on James Gober's explicit go; until then this file is the record of where the audit stands.

## What 1.0 promises

- **Semantic versioning.** No breaking change to the public API until 2.0. New items, new `ErrorKind` variants, and new `#[non_exhaustive]` fields arrive in minor releases.
- **Formats.** The decoders accept exactly what zlib accepts (RFC 1950, 1951, 1952) and the RFC 7692 rules; the encoders produce streams every compliant decoder accepts. The compressed bytes for a given input and setting may change in a minor release (better compression is not a breaking change); decompressed output never does.
- **Stable error codes.** `ErrorKind::code()` strings never change once released.
- **Safety.** `#![forbid(unsafe_code)]` and zero runtime dependencies for the whole 1.x line.
- **MSRV.** 1.85 for 1.0. Raising it is a minor release at most once per six months, announced one release ahead, and never for a patch release.
- **Deprecation.** Anything removed in 2.0 is `#[deprecated]` with a note naming its replacement for at least one minor release first.
- **Support.** Security fixes for the latest 1.x minor release; a reported vulnerability gets a fix or a published advisory.

## The audit

| Area | Item | Status |
|---|---|---|
| API | Every item in `docs/API.md` exists with the documented signature, and nothing public is undocumented | done (0.9): `docs/API.md` checked against the build item by item; `deny(missing_docs)` |
| API | Names are plain words; Tier 1 needs no configuration; types that must not be ignored are `#[must_use]`; extensible enums and structs are `#[non_exhaustive]` | done (0.9): `io::Encoder` renamed `io::EncodeReader` (it read like the root `Encoder`, D-051); `#[must_use]` added to the `io` constructors |
| API | Every engine is `Send` and `Sync` (compile-time test) | done (0.8) |
| Docs | Every public item has rustdoc with a runnable example; README, `docs/API.md`, and the rustdoc match the build | done (0.9): every public item has a runnable example (checked by script and by 225 doc tests) |
| Docs | `ARCHITECTURE.md`, `PERFORMANCE.md`, `SECURITY.md`, `PLATFORM-NOTES.md`, this file | done (0.9) |
| Tests | Unit tests for every `Ok` and `Err` path; property tests for every invariant; every edge case E-1 to E-30 | done (0.9): edge cases E-1 to E-30 mapped to tests in `dev/PLAN.md` section 10 |
| Tests | Differential tests against zlib (zlib-rs) and miniz_oxide; interop with C zlib through CPython in CI | done (0.3 to 0.8) |
| Security | An attack test per threat T-1 to T-14 (`tests/attacks.rs`) | done (0.7) |
| Security | 24-hour fuzz run of the inflater targets with no finding | AUDIT_FUZZ |
| Security | 4-hour soak with no heap growth | AUDIT_SOAK |
| Performance | Targets met or the gap published (`docs/PERFORMANCE.md`, D-045) | done (0.6), gap recorded |
| Platforms | CI green on Linux, macOS, Windows (x86_64 and aarch64), stable and 1.85; cross checks; Miri on big-endian | done |
| Consumers | ws-net, http-net, and serve-net paths exercised end to end (`tests/consumers.rs`) | done (0.8) |
| Release | James's go for 1.0 | waiting |

## Known limits carried into 1.0

- Compression speed at levels 6 and 9 is behind zlib-rs, which uses SIMD and `unsafe` (D-045). A different match finder is a 1.x candidate; it would change compressed bytes, not the API.
- Brotli is planned for 1.1 (decode) and 1.2 (encode), behind a feature, without API changes to the DEFLATE family.
