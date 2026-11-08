# Platform notes

compress-lib calls no operating system interface: the codecs are pure computation over slices, so they behave the same on every platform. This file records what is checked, where, and the few differences that exist.

## Checked in CI

| Target | How |
|---|---|
| Linux x86_64 and aarch64, macOS aarch64 and x86_64, Windows x86_64 and aarch64 | full test suite, stable and MSRV 1.85, default, all, and no default features |
| i686 Linux (32-bit) | `cargo check`, stable and 1.85 |
| s390x Linux (big-endian) | `cargo check`, and the bit reader, table builder, checksums, and inflater tests under Miri |
| wasm32-unknown-unknown | `cargo check` |
| thumbv7em-none-eabi (bare metal, `no_std`) | `cargo check --no-default-features` |

## Determinism

The same input, level, window, and flush sequence give the same compressed bytes on every platform and pointer width (NFR-7): the encoder uses no platform-dependent arithmetic, no hash randomisation, and no floating point, and gzip headers default to time stamp 0 and OS 255. The test suite checks that output does not depend on how input was split; the hosted benchmarks print the same compressed sizes on Linux, macOS, and Windows.

## Byte order and width

Multi-byte reads assemble values with `from_le_bytes`, so big-endian targets decode the same bits; Miri runs the decoder on s390x on every push. Lengths and counters use `u64` for totals and `usize` only for in-memory sizes, so 32-bit targets decode streams of any length (totals wrap only past 2^64; gzip's ISIZE is the length modulo 2^32, as RFC 1952 defines it).

## `no_std`

Without the default `std` feature the crate is `no_std` with `alloc`: everything except the `io` adapters and `From<Error> for std::io::Error`. `core::error::Error` is implemented in both modes.

## Memory allocation

Engines allocate when they are built and, for the vector APIs, when the output vector grows; vector growth uses `try_reserve`, so an allocation failure is `AllocationFailed`, not an abort. The slice APIs allocate nothing after construction (counting-allocator test).

## Performance by platform

See `docs/PERFORMANCE.md`: numbers for Linux x86_64, macOS on Apple silicon, and Windows x86_64 from hosted runners. No platform-specific code paths exist, so differences are the CPU and compiler, not the crate.
