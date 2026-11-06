# compress-lib - Engineering Directives

> Engineering standards and the definition of done for this project. Read alongside `REPS.md` (root, authoritative), [`PLAN.md`](./PLAN.md) (design and decisions), and [`ROADMAP.md`](./ROADMAP.md) (current phase). If anything here conflicts with `REPS.md`, `REPS.md` wins, except where `PLAN.md` records an approved exception (D-005: no async runtime).

---

## 0. Philosophy

This library is built and maintained to a production standard. Plan the full path, then build one verified step at a time. "Good enough" is treated as a defect. It is a `-lib`: codecs and plain values, with no threads, no timers, and no I/O in the core. It sits under WebSocket, HTTP, and file servers that decode hostile input all day, so it must be correct on every input, bounded on every input, and fast. A security component that cannot name the attack it stops, and prove it with a test, does not ship.

---

## 1. What this is

compress-lib compresses and decompresses raw DEFLATE, zlib, and gzip, and carries the per-message rules of WebSocket permessage-deflate. Brotli follows after 1.0. See `PLAN.md` sections 1 to 3.

---

## 2. Engineering law (non-negotiable)

- **Ours, not rented.** No third-party runtime dependency in any feature mode; CI fails on one. Dev-only tooling (`proptest`, `miniz_oxide` as a test oracle) is allowed; competitor crates live only in `bench-compare/`, which is never published.
- **No unsafe.** `#![forbid(unsafe_code)]`.
- **Sans-IO.** The core takes slices and returns progress. It never reads a socket, a file, or a clock. The `std::io` adapters are thin wrappers.
- **Bounded.** Memory is fixed when an engine is built and stated per configuration. Every decoder checks its output caps per chunk, before writing. Work per call is linear in bytes consumed and produced; no quadratic path on any input or buffer size.
- **Fail closed.** Defaults are safe (64 MiB output cap, checksums always verified, trailing data rejected). A failed engine stays failed until `reset()`.
- **Hostile-input hardening.** Every value from input goes through checked arithmetic or an explicit bound; no recursion; no reachable panic; every decoder has a fuzz target and differential tests.
- **Deterministic.** The same input and settings give the same bytes on every platform and pointer width.
- **Performance.** No allocation on hot paths (counting allocator); no "faster" claim without criterion numbers; the bar is at least 20% faster than the best safe-Rust competitor at equal or better ratio.
- **Error handling.** Every fallible path returns `Result`; `ErrorKind` is `#[non_exhaustive]`; every kind says what to do and has a stable code.
- **Production-ready.** `#![deny(missing_docs)]`; no `unwrap`/`expect`/`panic!`/`todo!`/`unimplemented!`/`unreachable!`/`dbg!`/printing in library code; every public item has rustdoc with a runnable example; every claim in the docs maps to a test.

---

## 3. Definition of done

1. Compiles clean on Linux, macOS, and Windows (x86_64 and aarch64), stable and MSRV 1.85; cross checks pass (i686, s390x, wasm32, thumbv7em `no_std`).
2. `fmt`, `clippy -D warnings` (all targets; default, all, and no default features), tests in the same three modes, `cargo doc -D warnings` with default and all features, `cargo package`: clean on Windows and in WSL Ubuntu, stable and 1.85.
3. `cargo audit` and `cargo deny check` pass; `cargo tree -e normal` shows the crate alone.
4. A Tier 1 API exists and headlines the docs.
5. Every decoder has a fuzz target; every threat in `PLAN.md` section 8 has an attack test; every edge case the change touches has a test; differential tests against miniz_oxide pass.
6. Hot-path changes carry benchmarks; no regression over 5%.
7. `README.md`, `docs/API.md`, and `CHANGELOG.md` updated in the same change; `docs/release/vX.Y.Z.md` written before the tag; every version string bumped.
8. `PLAN.md` decision log updated for every design decision; one new defence idea per version.
9. v1.0.0 only after the full audit is recorded in `docs/STABILITY-1.0.md` and James says go.

---

## 4. Project-specific invariants

- `inflate(deflate(x)) == x` for every input, level, window size, and flush sequence.
- An inflater never writes past `max_output`, never emits a byte from outside the window, and never accepts a stream zlib would reject (differential tests).
- A deflater never emits a distance above `2^window_bits` or across a full flush.
- `Progress::consumed <= input.len()` and `Progress::produced <= output.len()` on every call; a call that returns `NeedsInput` has consumed all input.
- The output for a given input does not depend on how the input was split or how large the output buffers were.
- Nothing panics on any input.

---

## 5. Writing style

- Plain, specific, human prose in every shipping file. No marketing voice.
- Banned in shipping files: "comprehensive", "robust", "seamless", "leverage", the em-dash character (U+2014), and emoji.
- Every security measure states its cost (latency, memory); estimates are labelled as estimates, measurements as measured.
- Release notes in the past tense.
- Commits: imperative present tense, descriptive subject line.
