# Performance

Measured, not asserted. Every number here comes from the `Bench` workflow (`.github/workflows/bench.yml`), which runs `bench-compare/` on GitHub-hosted runners: `ubuntu-latest` (x86_64), `macos-latest` (Apple silicon), and `windows-latest` (x86_64). Run of 2026-11-09, compress-lib 1.0.0, against miniz_oxide 0.8, zlib-rs 0.6, and flate2 1.x over C zlib (Linux, macOS) or zlib-rs (Windows). Hosted runners are shared machines: read differences under about 10% as noise, and compare across platforms with care (the same job can land on a slower machine from one run to the next).

## Method

- Inputs are generated deterministically by the benchmark (no downloads): English-like text, JSON records, HTML-like markup, structured binary records, and random bytes, 1 MiB each.
- Throughput is uncompressed bytes per second (MB/s, 10^6 bytes), the criterion median, one thread, release profile with fat LTO.
- Raw DEFLATE everywhere (no framing), so every library does the same work. Decoders decode streams made by zlib-rs.
- Cells read `compress-lib MB/s (ratio to miniz_oxide / ratio to zlib-rs)`; above 1.00 means compress-lib is faster.
- compress-lib is `#![forbid(unsafe_code)]` with no SIMD intrinsics; zlib-rs uses `unsafe` and SIMD; miniz_oxide forbids `unsafe` in its core.
- Before criterion, the same job runs the `quick` tool (`bench-compare/src/bin/quick.rs`), which times this checkout against the last release (0.9.0) and miniz_oxide in interleaved rounds, so slow and fast moments of the shared machine fall on every side alike.

## Decompression

| Input, level | Linux x86_64 | macOS aarch64 | Windows x86_64 |
|---|---|---|---|
| text, made at level 1 | 650 (1.42x / 0.87x) | 621 (1.86x / 1.11x) | 482 (2.04x / 1.02x) |
| text, made at level 6 | 805 (1.39x / 0.92x) | 801 (2.06x / 0.94x) | 623 (1.86x / 1.10x) |
| json, made at level 1 | 1,018 (1.20x / 0.96x) | 1,053 (1.59x / 1.07x) | 664 (1.38x / 1.06x) |
| json, made at level 6 | 1,287 (1.17x / 0.95x) | 1,374 (1.77x / 1.01x) | 692 (1.20x / 0.88x) |
| html, made at level 1 | 1,019 (1.26x / 0.99x) | 1,124 (2.00x / 1.06x) | 750 (1.53x / 1.11x) |
| html, made at level 6 | 1,378 (1.15x / 1.05x) | 1,493 (1.95x / 0.96x) | 822 (1.16x / 0.96x) |
| binary, made at level 1 | 481 (1.13x / 0.84x) | 518 (1.36x / 1.14x) | 379 (1.13x / 0.95x) |
| binary, made at level 6 | 447 (1.10x / 0.84x) | 478 (1.29x / 1.07x) | 350 (1.14x / 0.92x) |
| random, made at level 1 | 383 (1.07x / 0.88x) | 357 (0.98x / 1.06x) | 296 (0.98x / 0.95x) |
| random, made at level 6 | 29,284 (1.34x / 1.04x) | 28,829 (1.20x / 1.01x) | 3,869 (0.94x / 0.94x) |

Decoding is faster than miniz_oxide on every compressible row on every platform (1.10 to 2.06 times) and level with zlib-rs (0.84 to 1.14 times). Random input is stored blocks, so decoding it is a copy: 0.94 to 1.34 times miniz_oxide.

## Compression

| Input, level | Linux x86_64 | macOS aarch64 | Windows x86_64 |
|---|---|---|---|
| text, level 1 | 161 (0.76x / 0.62x) | 170 (0.89x / 1.18x) | 131 (0.77x / 0.63x) |
| text, level 6 | 13 (1.20x / 0.48x) | 19 (1.19x / 0.49x) | 12 (1.21x / 0.50x) |
| text, level 9 | 3.3 (0.97x / 0.38x) | 6.7 (1.10x / 0.51x) | 3.1 (0.97x / 0.39x) |
| json, level 1 | 321 (0.93x / 0.60x) | 341 (1.35x / 1.11x) | 265 (0.92x / 0.64x) |
| json, level 6 | 70 (1.61x / 0.67x) | 101 (1.47x / 0.82x) | 62 (1.57x / 0.70x) |
| json, level 9 | 19 (0.82x / 0.34x) | 32 (0.87x / 0.44x) | 17 (0.81x / 0.35x) |
| html, level 1 | 308 (0.92x / 0.62x) | 306 (0.91x / 0.96x) | 255 (0.92x / 0.65x) |
| html, level 6 | 52 (1.58x / 0.74x) | 80 (1.71x / 1.01x) | 46 (1.52x / 0.73x) |
| html, level 9 | 13 (0.88x / 0.31x) | 18 (0.95x / 0.35x) | 12 (0.89x / 0.32x) |
| binary, level 1 | 180 (0.81x / 0.63x) | 147 (0.69x / 0.96x) | 142 (0.94x / 0.62x) |
| binary, level 6 | 10 (1.00x / 0.23x) | 16 (1.29x / 0.37x) | 9.8 (1.05x / 0.25x) |
| binary, level 9 | 9.5 (1.04x / 0.71x) | 10 (1.18x / 0.87x) | 8.8 (1.09x / 0.72x) |
| random, level 1 | 454 (1.95x / 4.02x) | 389 (1.64x / 4.13x) | 301 (1.91x / 3.56x) |
| random, level 6 | 59 (1.39x / 0.99x) | 59 (1.43x / 0.97x) | 47 (1.36x / 1.08x) |
| random, level 9 | 59 (1.40x / 1.09x) | 57 (1.30x / 1.13x) | 49 (1.35x / 1.13x) |

## Since 0.9.0

From the `quick` tool in the same jobs (same machine, interleaved): compression is 0.99 to 1.31 times as fast as 0.9.0 across all five inputs at levels 1, 6, and 9 (the level-1 loop, D-055, and the block writer's per-block tables, D-056), with the same compressed bytes at levels 6 and 9 and level 1 up to 2.5% smaller. Decoding is 1.06 to 1.28 times 0.9.0 on Windows (2.5 times on stored random data; D-058) and level with it on Linux and macOS (0.93 to 1.17 times, within noise).

## Messages and gzip

| | Linux x86_64 | macOS aarch64 | Windows x86_64 |
|---|---|---|---|
| 200 JSON messages, no takeover, level 1 | 31 (4.31x) | 29 (1.62x) | 23 (1.66x) |
| gzip, 1 MiB JSON: compress / decompress | 68 / 853 | 72 / 873 | 60 / 563 |

## Compressed sizes (bytes, 1 MiB inputs)

| Input | Level | compress-lib | miniz_oxide | zlib-rs | C zlib (flate2) |
|---|---|---|---|---|---|
| text | 1 | 394,107 | 429,161 | 555,999 | 384,545 |
| text | 6 | 309,621 | 310,702 | 301,237 | 309,608 |
| text | 9 | 295,023 | 295,246 | 295,073 | 295,023 |
| json | 1 | 227,625 | 222,090 | 272,524 | 213,610 |
| json | 6 | 165,750 | 159,840 | 167,480 | 165,759 |
| json | 9 | 155,618 | 157,743 | 158,150 | 155,620 |
| html | 1 | 243,798 | 242,174 | 280,542 | 225,921 |
| html | 6 | 160,030 | 152,440 | 159,250 | 160,031 |
| html | 9 | 147,178 | 148,779 | 149,798 | 147,179 |
| binary | 1 | 605,670 | 605,755 | 707,489 | 601,468 |
| binary | 6 | 601,149 | 603,235 | 595,529 | 601,238 |
| binary | 9 | 595,599 | 596,689 | 595,860 | 595,600 |
| random | 1 | 1,048,896 | 1,049,563 | 1,105,923 | 1,048,896 |
| random | 6 | 1,048,896 | 1,048,746 | 1,048,901 | 1,048,896 |
| random | 9 | 1,048,896 | 1,048,746 | 1,048,896 | 1,048,896 |

At levels 6 and 9 the output is within a few bytes of C zlib's, as it should be with zlib's match-finder settings (D-026). Level 1 is a single-probe matcher (D-040, D-055): smaller than zlib-rs's level 1 everywhere, 8% smaller than miniz_oxide's on text and within 2.5% on the rest.

## Against the targets (plan section 9)

| Target | Result |
|---|---|
| Inflate at least 1.2 times miniz_oxide | met on text everywhere (1.39 to 2.06 times), on every macOS row but random input, and on JSON and HTML made at level 1 (1.20 to 2.00 times); 1.10 to 1.17 times on binary input and on JSON and HTML made at level 6 on Linux and Windows; random input (a copy) 0.94 to 1.34 times |
| Inflate parity with zlib-rs (stretch) | met within noise (0.84 to 1.14 times) |
| Deflate level 1 at least 1.2 times miniz_oxide at equal or better ratio | not met on compressible input: 0.69 to 0.94 times on Linux and Windows, 0.69 to 1.35 times on macOS; 1.64 to 1.95 times on random input; output 8% smaller than miniz_oxide's on text, within 2.5% on the rest |
| Deflate level 6 at least 1.2 times miniz_oxide, ratio within 1% | met on JSON, HTML, and random input (1.36 to 1.71 times) and on text within noise (1.19 to 1.21 times); binary 1.00 to 1.29 times; ratio within 0.01% of C zlib |
| Deflate level 9 ratio within 0.5% of zlib, speed at least miniz_oxide's | ratio met (equal to C zlib); speed met on binary and random input (1.04 to 1.40 times) and on text within noise (0.97 to 1.10 times), not on JSON and HTML (0.81 to 0.95 times, with output about 1% smaller than miniz_oxide's) |
| Per-message compression below miniz_oxide's cost | met: 1.62 to 4.31 times miniz_oxide |

The gap is compression speed (D-045, D-061). The 1.0 polish pass sped compression up by up to a third over 0.9 without changing a byte of output at levels 6 and 9, but what remains is structural: level 1 needs a writer that codes symbols while it matches instead of buffering them, and levels 8 and 9 need a binary-tree match finder (as libdeflate uses). Both change compressed bytes, not the API, so they are 1.x candidates. Against zlib-rs, which uses SIMD and `unsafe`, compression is 0.23 to 0.87 times at levels 6 and 9 on compressible input; OD-1 records why `unsafe` is not the answer by default.

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
