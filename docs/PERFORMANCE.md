# Performance

Measured, not asserted. Every number here comes from the `Bench` workflow (`.github/workflows/bench.yml`), which runs `bench-compare/` on GitHub-hosted runners: `ubuntu-latest` (x86_64), `macos-latest` (Apple silicon), and `windows-latest` (x86_64). Run of 2026-11-12, compress-lib 1.1.0, against miniz_oxide 0.8, zlib-rs 0.6, and flate2 1.x over C zlib (Linux, macOS) or zlib-rs (Windows). Hosted runners are shared machines: read differences under about 10% as noise, and compare across platforms with care (the same job can land on a slower machine from one run to the next).

## Method

- Inputs are generated deterministically by the benchmark (no downloads): English-like text, JSON records, HTML-like markup, structured binary records, and random bytes, 1 MiB each.
- Throughput is uncompressed bytes per second (MB/s, 10^6 bytes), the criterion median, one thread, release profile with fat LTO.
- Raw DEFLATE everywhere (no framing), so every library does the same work. Decoders decode streams made by zlib-rs.
- Cells read `compress-lib MB/s (ratio to miniz_oxide / ratio to zlib-rs)`; above 1.00 means compress-lib is faster.
- compress-lib is `#![forbid(unsafe_code)]` with no SIMD intrinsics; zlib-rs uses `unsafe` and SIMD; miniz_oxide forbids `unsafe` in its core.
- Before criterion, the same job runs the `quick` tool (`bench-compare/src/bin/quick.rs`), which times this checkout against the last release (1.0.0 since 1.1) and miniz_oxide in interleaved rounds, so slow and fast moments of the shared machine fall on every side alike.

## Decompression

| Input, level | Linux x86_64 | macOS aarch64 | Windows x86_64 |
|---|---|---|---|
| text, made at level 1 | 641 (1.39x / 0.87x) | 628 (2.15x / 1.07x) | 715 (1.53x / 1.04x) |
| text, made at level 6 | 790 (1.38x / 0.92x) | 832 (2.10x / 0.94x) | 792 (1.10x / 1.02x) |
| json, made at level 1 | 1,015 (1.21x / 0.98x) | 1,087 (1.59x / 1.04x) | 958 (1.25x / 1.06x) |
| json, made at level 6 | 1,280 (1.16x / 0.96x) | 1,444 (1.81x / 1.11x) | 1,113 (1.21x / 0.92x) |
| html, made at level 1 | 1,007 (1.24x / 1.00x) | 1,096 (1.84x / 1.08x) | 985 (1.21x / 1.09x) |
| html, made at level 6 | 1,334 (1.13x / 1.03x) | 1,480 (1.93x / 0.96x) | 1,137 (1.08x / 1.00x) |
| binary, made at level 1 | 477 (1.13x / 0.85x) | 508 (1.31x / 1.13x) | 507 (1.24x / 0.97x) |
| binary, made at level 6 | 441 (1.09x / 0.84x) | 470 (1.26x / 1.10x) | 465 (1.20x / 0.94x) |
| random, made at level 1 | 383 (1.07x / 0.87x) | 361 (1.02x / 1.15x) | 393 (1.09x / 0.86x) |
| random, made at level 6 | 29,788 (1.32x / 1.07x) | 31,780 (1.24x / 1.14x) | 5,005 (0.90x / 0.92x) |

On these corpora and runners, decoding is faster than miniz_oxide on every compressible row (1.10 to 2.08 times) and level with zlib-rs (0.75 to 1.17 times). Random input is stored blocks, so decoding it is a copy: 0.95 to 1.50 times miniz_oxide. An external review measured streams made by zlib-rs at level 1 (fixed-Huffman blocks) decoding at 0.81 to 0.94 times miniz_oxide on a loaded Windows machine with other corpora; the hosted runs here do not show it, and a wider fast path for fixed blocks is a 1.x candidate.

## Compression

| Input, level | Linux x86_64 | macOS aarch64 | Windows x86_64 |
|---|---|---|---|
| text, level 1 | 158 (0.76x / 0.61x) | 121 (0.72x / 0.77x) | 166 (0.70x / 0.58x) |
| text, level 6 | 18 (1.61x / 0.66x) | 20 (1.51x / 0.61x) | 24 (1.58x / 0.70x) |
| text, level 9 | 6.6 (1.94x / 0.76x) | 8.4 (1.83x / 0.71x) | 8.9 (2.02x / 0.82x) |
| json, level 1 | 315 (0.91x / 0.60x) | 282 (0.99x / 0.92x) | 348 (0.94x / 0.59x) |
| json, level 6 | 80 (1.84x / 0.76x) | 82 (1.73x / 0.76x) | 102 (1.78x / 0.80x) |
| json, level 9 | 23 (1.00x / 0.41x) | 25 (0.74x / 0.37x) | 34 (1.08x / 0.51x) |
| html, level 1 | 305 (0.90x / 0.62x) | 206 (0.70x / 0.75x) | 325 (0.88x / 0.59x) |
| html, level 6 | 54 (1.65x / 0.77x) | 56 (1.83x / 0.72x) | 69 (1.62x / 0.85x) |
| html, level 9 | 13 (0.92x / 0.33x) | 14 (0.83x / 0.28x) | 18 (0.94x / 0.40x) |
| binary, level 1 | 186 (0.83x / 0.65x) | 170 (0.74x / 0.97x) | 220 (0.77x / 0.67x) |
| binary, level 6 | 17 (1.67x / 0.38x) | 24 (1.87x / 0.47x) | 22 (1.68x / 0.42x) |
| binary, level 9 | 17 (1.91x / 1.28x) | 26 (2.25x / 1.57x) | 22 (1.98x / 1.40x) |
| random, level 1 | 431 (1.88x / 3.70x) | 406 (1.46x / 4.06x) | 464 (1.64x / 3.80x) |
| random, level 6 | 59 (1.44x / 1.02x) | 64 (1.52x / 1.18x) | 65 (1.36x / 1.01x) |
| random, level 9 | 58 (1.44x / 1.09x) | 61 (1.27x / 1.09x) | 66 (1.34x / 1.03x) |

## Since 1.0

Levels 4 to 9 hash four bytes instead of three and search within a budget of chain steps per input byte (D-065). Against 1.0.0 the bytes at levels 4 to 9 differ and are smaller on every corpus (level 6: 0.6 to 2.9 percent; level 9: up to 1.1 percent; levels 4 and 5: up to 10 percent), and the `quick` tool in the same jobs measures levels 6 and 9 at 1.09 to 2.79 times 0.9.0, whose bytes and speed at those levels are 1.0's within noise. On structured binary data level 6 went from about a quarter of zlib-rs's speed to over a third. On input crafted to keep every hash chain full, level 9 runs 3.6 to 30 times as fast as in 1.0 (1.9 to 24 MB/s locally, up from 0.4 to 0.9). Levels 0 to 3 are unchanged.

## Messages and gzip

| | Linux x86_64 | macOS aarch64 | Windows x86_64 |
|---|---|---|---|
| 200 JSON messages, no takeover, level 1 | 31 (4.34x) | 30 (1.65x) | 36 (1.25x) |
| gzip, 1 MiB JSON: compress / decompress | 77 / 848 | 90 / 966 | 94 / 769 |

## Compressed sizes (bytes, 1 MiB inputs)

| Input | Level | compress-lib | miniz_oxide | zlib-rs | C zlib (flate2) |
|---|---|---|---|---|---|
| text | 1 | 394,107 | 429,161 | 555,999 | 384,545 |
| text | 6 | 300,781 | 310,702 | 301,237 | 309,608 |
| text | 9 | 294,785 | 295,246 | 295,073 | 295,023 |
| json | 1 | 227,625 | 222,090 | 272,524 | 213,610 |
| json | 6 | 163,856 | 159,840 | 167,480 | 165,759 |
| json | 9 | 153,850 | 157,743 | 158,150 | 155,620 |
| html | 1 | 243,798 | 242,174 | 280,542 | 225,921 |
| html | 6 | 159,084 | 152,440 | 159,250 | 160,031 |
| html | 9 | 146,027 | 148,779 | 149,798 | 147,179 |
| binary | 1 | 605,670 | 605,755 | 707,489 | 601,468 |
| binary | 6 | 595,590 | 603,235 | 595,529 | 601,238 |
| binary | 9 | 595,590 | 596,689 | 595,860 | 595,600 |
| random | 1 | 1,048,896 | 1,049,563 | 1,105,923 | 1,048,896 |
| random | 6 | 1,048,896 | 1,048,746 | 1,048,901 | 1,048,896 |
| random | 9 | 1,048,896 | 1,048,746 | 1,048,896 | 1,048,896 |

At levels 6 and 9 the output is now smaller than C zlib's on text, JSON, HTML, and binary input (up to 2.9 percent at level 6): with a four-byte hash the search emits far fewer three-byte matches, which at these levels often stand in the way of a longer match a byte later (D-065). Level 1 is a single-probe matcher (D-040, D-055): smaller than zlib-rs's level 1 everywhere, 8% smaller than miniz_oxide's on text and within 2.5% on the rest.

## Against the targets (plan section 9)

| Target | Result |
|---|---|
| Inflate at least 1.2 times miniz_oxide | met on text everywhere (1.38 to 2.08 times), on every macOS compressible row, and on JSON and HTML made at level 1 (1.22 to 1.90 times); 1.10 to 1.17 times on binary input and on JSON and HTML made at level 6 on Linux and Windows; random input (a copy) 0.95 to 1.50 times |
| Inflate parity with zlib-rs (stretch) | met within noise on most rows (0.87 to 1.17 times); JSON made at level 6 on macOS 0.75 times |
| Deflate level 1 at least 1.2 times miniz_oxide at equal or better ratio | not met on compressible input: 0.77 to 1.07 times; 1.66 to 2.21 times on random input; output 8% smaller than miniz_oxide's on text, within 2.5% on the rest |
| Deflate level 6 at least 1.2 times miniz_oxide, ratio within 1% | met: 1.63 to 2.25 times on text, JSON, HTML, and binary, 1.24 to 1.36 on random input; output smaller than C zlib's |
| Deflate level 9 ratio within 0.5% of zlib, speed at least miniz_oxide's | ratio met (smaller than C zlib's); speed met on text, binary, and random input (1.33 to 2.28 times) and within noise of it on JSON and HTML (0.91 to 1.12 times) |
| Per-message compression below miniz_oxide's cost | met: 1.65 to 1.78 times miniz_oxide |

The gap left is level 1, level 9 on JSON and HTML, and compression against zlib-rs, which uses SIMD and `unsafe`: 0.61 to 0.85 times at level 6 on text, JSON, and HTML, 0.38 to 0.47 on binary, and 0.28 to 0.51 at level 9 on JSON and HTML (D-061). Level 1 needs a writer that codes symbols while it matches; levels 8 and 9 on JSON and HTML need a binary-tree match finder. Both are 1.x candidates; OD-1 records why `unsafe` is not the answer by default.

## Memory

Measured with `memory_in_use()` (heap bytes held; the same on every platform):

| Engine | Window bits 15 | Window bits 9 |
|---|---|---|
| `Inflater` | 44,576 | 12,320 |
| `message::Decompressor` without context takeover (no window) | 11,808 | 11,808 |
| `message::Decompressor` with takeover | 44,576 | 12,320 |
| `Deflater` level 1, `message::Compressor` (level 1) | 338,936 | 30,712 |
| `Deflater` level 6 | 396,280 | 30,712 |

A `Deflater` holds its window buffer (at least 8 KiB, two windows for bits 12 and up), the hash table and chains, the symbol buffer, and room for at least one block of pending output; `MemoryConfig` sizes the hash table and the symbol buffer. Since 1.1 these figures hold while compressing too: before, blocks could pile up in the pending output and a level 2 to 9 deflater could grow by a third (`test_deflater_memory_never_grows`).

## Reproducing

```text
cd bench-compare
cargo bench                          # compress-lib, miniz_oxide, zlib-rs
cargo bench --features flate2-zlib   # plus flate2 over C zlib
```
