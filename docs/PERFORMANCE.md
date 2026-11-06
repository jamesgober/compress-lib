# Performance

Measured, not asserted. Every number here comes from the `Bench` workflow (`.github/workflows/bench.yml`), which runs `bench-compare/` on GitHub-hosted runners: `ubuntu-latest` (x86_64), `macos-latest` (Apple silicon), and `windows-latest` (x86_64). Run of 2026-11-06, compress-lib 0.6.0, against miniz_oxide 0.8, zlib-rs 0.6, and flate2 1.x over C zlib (Linux, macOS) or zlib-rs (Windows). Hosted runners are shared machines: read differences under about 10% as noise.

## Method

- Inputs are generated deterministically by the benchmark (no downloads): English-like text, JSON records, HTML-like markup, structured binary records, and random bytes, 1 MiB each.
- Throughput is uncompressed bytes per second (MB/s, 10^6 bytes), the criterion median, one thread, release profile with fat LTO.
- Raw DEFLATE everywhere (no framing), so every library does the same work. Decoders decode streams made by zlib-rs.
- Cells read `compress-lib MB/s (ratio to miniz_oxide / ratio to zlib-rs)`; above 1.00 means compress-lib is faster.
- compress-lib is `#![forbid(unsafe_code)]` with no SIMD intrinsics; zlib-rs uses `unsafe` and SIMD; miniz_oxide forbids `unsafe` in its core.

## Decompression

| Input, level | Linux x86_64 | macOS aarch64 | Windows x86_64 |
|---|---|---|---|
| text, made at level 1 | 508 (1.34x / 0.87x) | 540 (2.09x / 1.23x) | 577 (1.82x / 0.97x) |
| text, made at level 6 | 673 (1.37x / 0.94x) | 787 (1.99x / 0.95x) | 700 (1.66x / 0.99x) |
| json, made at level 1 | 842 (1.18x / 0.96x) | 954 (1.53x / 1.12x) | 848 (1.27x / 1.02x) |
| json, made at level 6 | 1,061 (1.16x / 0.95x) | 971 (1.51x / 0.82x) | 1,035 (1.32x / 1.02x) |
| html, made at level 1 | 831 (1.22x / 0.98x) | 1,006 (1.96x / 1.15x) | 847 (1.42x / 1.05x) |
| html, made at level 6 | 1,134 (1.12x / 1.04x) | 938 (1.50x / 0.70x) | 1,061 (1.29x / 1.15x) |
| binary, made at level 1 | 401 (1.11x / 0.87x) | 452 (1.22x / 1.07x) | 422 (1.13x / 0.89x) |
| binary, made at level 6 | 370 (1.07x / 0.86x) | 407 (1.33x / 0.99x) | 400 (1.09x / 0.86x) |
| random, made at level 1 | 338 (1.08x / 0.96x) | 305 (1.02x / 1.22x) | 358 (1.03x / 0.85x) |
| random, made at level 6 | 19,994 (1.48x / 1.13x) | 20,565 (1.24x / 1.01x) | 4,641 (0.98x / 0.94x) |

Decoding is faster than miniz_oxide on every row on every platform but one (random data on Windows, even), and level with zlib-rs (0.86 to 1.23 times).

## Compression

| Input, level | Linux x86_64 | macOS aarch64 | Windows x86_64 |
|---|---|---|---|
| text, level 1 | 123 (0.67x / 0.56x) | 110 (0.71x / 0.74x) | 138 (0.68x / 0.56x) |
| text, level 6 | 12 (1.19x / 0.47x) | 14 (0.98x / 0.43x) | 12 (1.14x / 0.46x) |
| text, level 9 | 3 (0.99x / 0.38x) | 5 (0.85x / 0.45x) | 3 (0.96x / 0.37x) |
| json, level 1 | 265 (0.88x / 0.61x) | 315 (0.97x / 1.03x) | 278 (0.84x / 0.56x) |
| json, level 6 | 62 (1.61x / 0.68x) | 67 (1.74x / 0.68x) | 65 (1.50x / 0.63x) |
| json, level 9 | 17 (0.82x / 0.34x) | 20 (0.65x / 0.30x) | 17 (0.74x / 0.30x) |
| html, level 1 | 242 (0.83x / 0.58x) | 242 (0.82x / 0.72x) | 254 (0.78x / 0.54x) |
| html, level 6 | 48 (1.63x / 0.75x) | 61 (1.47x / 0.68x) | 50 (1.51x / 0.72x) |
| html, level 9 | 11 (0.89x / 0.31x) | 15 (0.86x / 0.29x) | 12 (0.80x / 0.29x) |
| binary, level 1 | 113 (0.62x / 0.47x) | 145 (0.63x / 0.83x) | 138 (0.66x / 0.51x) |
| binary, level 6 | 10 (1.12x / 0.25x) | 14 (1.02x / 0.29x) | 11 (1.03x / 0.24x) |
| binary, level 9 | 9 (1.20x / 0.76x) | 12 (1.02x / 0.73x) | 10 (1.07x / 0.72x) |
| random, level 1 | 324 (1.81x / 3.25x) | 359 (1.36x / 3.82x) | 307 (1.50x / 2.77x) |
| random, level 6 | 47 (1.27x / 0.92x) | 50 (1.10x / 0.93x) | 50 (1.20x / 0.91x) |
| random, level 9 | 47 (1.27x / 1.00x) | 49 (1.08x / 1.04x) | 50 (1.18x / 0.93x) |

## Messages and gzip

| | Linux x86_64 | macOS aarch64 | Windows x86_64 |
|---|---|---|---|
| 200 JSON messages, no takeover, level 1 | 25 (1.84x) | 25 (1.66x) | 33 (4.61x) |
| gzip, 1 MiB JSON: compress / decompress | 60 / 719 | 69 / 739 | 65 / 718 |

## Compressed sizes (bytes, 1 MiB inputs)

| Input | Level | compress-lib | miniz_oxide | zlib-rs | C zlib (flate2) |
|---|---|---|---|---|---|
| text | 1 | 394,553 | 429,161 | 555,999 | 384,545 |
| text | 6 | 309,621 | 310,702 | 301,237 | 309,608 |
| text | 9 | 295,023 | 295,246 | 295,073 | 295,023 |
| json | 1 | 233,045 | 222,090 | 272,524 | 213,610 |
| json | 6 | 165,750 | 159,840 | 167,480 | 165,759 |
| json | 9 | 155,618 | 157,743 | 158,150 | 155,620 |
| html | 1 | 250,032 | 242,174 | 280,542 | 225,921 |
| html | 6 | 160,030 | 152,440 | 159,250 | 160,031 |
| html | 9 | 147,178 | 148,779 | 149,798 | 147,179 |
| binary | 1 | 606,046 | 605,755 | 707,489 | 601,468 |
| binary | 6 | 601,149 | 603,235 | 595,529 | 601,238 |
| binary | 9 | 595,599 | 596,689 | 595,860 | 595,600 |
| random | 1 | 1,048,896 | 1,049,563 | 1,105,923 | 1,048,896 |
| random | 6 | 1,048,896 | 1,048,746 | 1,048,901 | 1,048,896 |
| random | 9 | 1,048,896 | 1,048,746 | 1,048,896 | 1,048,896 |

At levels 6 and 9 the output is within a few bytes of C zlib's, as it should be with zlib's match-finder settings (D-026). Level 1 is a single-probe matcher (D-040): smaller than zlib-rs's level 1 everywhere, close to miniz_oxide's.

## Against the targets (plan section 9)

| Target | Result |
|---|---|
| Inflate at least 1.2 times miniz_oxide | met on most rows (1.07 to 2.09 times); binary and random inputs on Linux and Windows are 1.02 to 1.13 times |
| Inflate parity with zlib-rs (stretch) | met within noise (0.86 to 1.23 times) |
| Deflate level 1 at least 1.2 times miniz_oxide at equal or better ratio | not met: 0.62 to 0.97 times on compressible input (1.36 to 1.81 times on random input); sizes are smaller than miniz_oxide's on text and close on the rest |
| Deflate level 6 at least 1.2 times miniz_oxide, ratio within 1% | met on JSON and HTML (1.47 to 1.74 times), not on text and binary (0.98 to 1.19 times); ratio within 1% of zlib |
| Deflate level 9 ratio within 0.5% of zlib, speed at least miniz_oxide's | ratio met (equal to C zlib); speed 0.65 to 1.20 times miniz_oxide |
| Per-message compression below miniz_oxide's cost | met: 1.66 to 4.61 times miniz_oxide |

The gap is compression speed (D-045). Against zlib-rs, which uses SIMD and `unsafe`, compression is 0.25 to 0.76 times at levels 6 and 9. Closing it within `forbid(unsafe_code)` needs a different match finder (binary trees, as libdeflate uses for its higher levels), which is a 1.x candidate; OD-1 records why `unsafe` is not the answer by default.

## Memory

Measured with `memory_in_use()` (heap bytes held; the same on every platform):

| Engine | Window bits 15 | Window bits 9 |
|---|---|---|
| `Inflater` | 44,576 | 12,320 |
| `message::Decompressor` without context takeover (no window) | 11,808 | 11,808 |
| `message::Decompressor` with takeover | 44,576 | 12,320 |
| `Deflater` level 1, `message::Compressor` (level 1) | 338,936 | 30,712 |
| `Deflater` level 6 | 396,280 | 30,712 |

A `Deflater` holds its window buffer (at least 8 KiB, two windows for bits 12 and up), the hash table and chains, the symbol buffer, and room for one block of pending output; `MemoryConfig` sizes the hash table and the symbol buffer.

## Reproducing

```text
cd bench-compare
cargo bench                          # compress-lib, miniz_oxide, zlib-rs
cargo bench --features flate2-zlib   # plus flate2 over C zlib
```
