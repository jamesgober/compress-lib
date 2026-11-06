# compress-lib - Project Plan

> The design, strategy, and decision record for `compress-lib`. A living document: every design decision, idea, or open question is written here or in [`ROADMAP.md`](./ROADMAP.md) before it is acted on.
>
> **Read order:** [`../REPS.md`](../REPS.md) &rarr; [`DIRECTIVES.md`](./DIRECTIVES.md) &rarr; **this file** &rarr; [`ROADMAP.md`](./ROADMAP.md).
>
> **Status:** `0.6.0` (2026-11-06): feature complete and measured ([`../docs/PERFORMANCE.md`](../docs/PERFORMANCE.md)); the public API is written out in [`../docs/API.md`](../docs/API.md) with the version of each item (D-010). Hardening is `0.7.0`.

---

## Contents

1. [Purpose](#1-purpose)
2. [Position in the stack](#2-position-in-the-stack)
3. [Scope](#3-scope)
4. [Requirements](#4-requirements)
5. [Design and architecture](#5-design-and-architecture)
6. [API specification](#6-api-specification)
7. [Parity matrix](#7-parity-matrix)
8. [Security](#8-security)
9. [Performance](#9-performance)
10. [Testing plan](#10-testing-plan)
11. [Documentation deliverables](#11-documentation-deliverables)
12. [Version roadmap](#12-version-roadmap)
13. [Definition of done](#13-definition-of-done)
14. [Open decisions](#14-open-decisions)
15. [Ideas parking lot](#15-ideas-parking-lot)
16. [Decision log](#16-decision-log)

---

## 1. Purpose

### 1.1 What it is

compress-lib compresses and decompresses the formats the web runs on, with one plain call for the common case and a streaming engine underneath for everything else:

```rust
let packed = compress::gzip::compress(body);              // Vec<u8>, level 6
let body = compress::gzip::decompress(&packed)?;          // capped at 64 MiB by default
let raw = compress::deflate::compress_with(data, compress::Level::FAST);
let data = compress::deflate::decompress_with(&raw, compress::Limits::new().with_max_output(1 << 20))?;
```

The engine is sans-IO: the caller pushes input bytes and pulls output bytes through buffers it owns, and the engine never reads a socket, a file, or a clock. Memory is fixed when the engine is built (a window of `2^window_bits` bytes plus tables) and never grows with input. Every decoder enforces an absolute output cap and an optional output-to-input ratio cap while it decodes, per output chunk, so a decompression bomb costs at most the cap.

Formats, in the order they ship:

1. Raw DEFLATE (RFC 1951), inflate and deflate, window bits 8 to 15, levels 0 to 9, flush modes none, sync, full, and finish.
2. RFC 7692 per-message rules on top of raw DEFLATE: strip and re-append the `00 00 FF FF` tail, context takeover on or off, window bits 8 to 15 in both directions.
3. zlib (RFC 1950) and gzip (RFC 1952) framing with Adler-32 and CRC-32, preset dictionaries, multi-member gzip, bounded gzip headers.
4. Brotli (RFC 7932) decode, then encode, after 1.0 (section 12).

### 1.2 What it is not

- **Not an I/O library.** `std::io::Read` and `Write` adapters exist behind the `std` feature for convenience; the engine itself takes slices.
- **Not an HTTP or WebSocket layer.** It does not parse `Content-Encoding`, `Accept-Encoding`, or `Sec-WebSocket-Extensions`. http-net, serve-net, and ws-net do; compress-lib gives them the codec and the per-message primitives.
- **Not an archive format.** No zip, tar, or 7z containers. A zip reader would use compress-lib's raw inflater; it is a separate crate if ever needed.
- **Not a binding.** No C, no `libz-sys`, no zlib-ng, no `unsafe`. The C libraries appear only in benchmarks.
- **Not zstd or lz4 (yet).** Listed P3 in section 3.2 with where they would live.

---

## 2. Position in the stack

### 2.1 Who uses it

| User | Uses | When |
|---|---|---|
| ws-net (feature `deflate`) | `message::Compressor`, `message::Decompressor` (raw DEFLATE, sync flush, tail handling, context takeover, window bits 8 to 15), decompress-into-`Vec` with a budget per chunk | permessage-deflate (ws-net FR-40 to FR-44), unblocks the move of deflate back into ws-net's 1.x line |
| http-net / fetch-net | `gzip`, `zlib`/`deflate` (HTTP's `deflate` coding is zlib, RFC 9110 section 8.4.1.2) streaming decoders with limits, encoders | `Content-Encoding` for request and response bodies |
| serve-net (direct mode) | streaming encoders for responses (`gzip`, `deflate`), level and minimum-size policy | when no Apache front end compresses |
| Hive DB, log shipping, backups | one-shot and streaming gzip | later, not driving the design |

### 2.2 What it uses

Nothing at run time. Dev-dependencies: `proptest` (properties), `miniz_oxide` (differential oracle in tests), and in the separate `bench-compare/` crate `criterion`, `miniz_oxide`, `zlib-rs` (through `libz-rs-sys`), and `flate2` with its backends.

### 2.3 Where it sits

A leaf: below ws-net, http-net, serve-net, above nothing. Stateless in the distributed sense (section 4.3).

---

## 3. Scope

### 3.1 In

Raw DEFLATE inflate and deflate; zlib and gzip framing; CRC-32 and Adler-32 (public, they are useful alone); RFC 7692 per-message helpers; limits (absolute output, ratio, gzip header fields, gzip members); preset dictionaries; `std::io` adapters; format detection for decoders (`Format::detect`); brotli after 1.0.

### 3.2 Out, and where it lives instead

| Item | Where |
|---|---|
| `Content-Encoding` negotiation, `Accept-Encoding` q-values | http-net / serve-net (parse-net lists) |
| `Sec-WebSocket-Extensions` negotiation | ws-net |
| zip, tar | a future archive crate on top of `deflate::Inflater` |
| zstd, lz4, snappy | P3; a later minor of this crate if a consumer needs one (zstd is the likely first, for Hive DB) |
| Async (`AsyncRead`/`AsyncWrite`) | callers wrap the sans-IO engine; no async runtime in this crate |

---

## 4. Requirements

### 4.1 Functional

| Id | Requirement | Version |
|---|---|---|
| FR-1 | Inflate any valid RFC 1951 stream: stored, fixed Huffman, and dynamic Huffman blocks, all lengths and distances, the full code-length alphabet. | 0.3 |
| FR-2 | Reject every invalid stream with a specific error: block type 3, stored LEN/NLEN mismatch, over-subscribed or incomplete code (except the single-code cases RFC 1951 permits), code-length repeat with no previous length or past the end, literal/length symbols 286 and 287, distance symbols 30 and 31, distance beyond the window or beyond the output produced plus dictionary, missing end-of-block code. | 0.3 |
| FR-3 | Streaming inflate: any split of the input and any size of output buffer (down to one byte) gives the same output as one call; the engine resumes in the middle of a header, a code, a length, a match, or a stored block. | 0.3 |
| FR-4 | Inflate window bits 8 to 15: the window is `2^bits` bytes and a distance above it is an error (RFC 7692 strict). | 0.3 |
| FR-5 | Output caps: an absolute total output cap and an optional ratio cap (output over input consumed, checked once output passes a floor), both checked before each output chunk is written, never after. | 0.3 |
| FR-6 | Inflate into a caller `Vec<u8>` (append), using the vector itself as history and reserving at most one chunk ahead of the cap. | 0.3 |
| FR-7 | Deflate at levels 0 (stored) to 9, with output any compliant inflater accepts, and ratio at the same level within the targets of section 9. | 0.4 |
| FR-8 | Deflate window bits 8 to 15: no distance above `2^bits` is ever emitted. | 0.4 |
| FR-9 | Flush modes: `None`, `Sync` (byte-align, empty stored block, `00 00 FF FF`), `Full` (sync and forget history), `Finish` (final block). | 0.4 |
| FR-10 | Per-message compression (RFC 7692 section 7.2): compress a message with sync flush and strip the trailing `00 00 FF FF`; decompress by appending the tail; context takeover on or off per direction; window bits 8 to 15 per direction; a final block (`BFINAL`) from the peer is accepted and the next message starts a new stream with the window kept when takeover is on. | 0.4 |
| FR-11 | Reset without freeing (`reset()`), new stream keeping history (`next_stream()`), preset dictionary (`set_dictionary`). | 0.4 |
| FR-12 | zlib: header check (CM 8, CINFO at most 7 and at most the configured window, FCHECK, FDICT with dictionary id), Adler-32 trailer verified; encoder writes FLEVEL. | 0.5 |
| FR-13 | gzip: header with FTEXT, FHCRC (verified), FEXTRA, FNAME, FCOMMENT, each bounded; CRC-32 and ISIZE verified; multi-member streams (on by default, limit on members); trailing bytes after the last member rejected by default, allowed by option; header fields readable. | 0.5 |
| FR-14 | Format detection for decoders: gzip by magic, zlib by header check, else error (raw cannot be detected and is never guessed). | 0.5 |
| FR-15 | `std::io` adapters: `Reader<R: Read>` (decompressing) and `Writer<W: Write>` (compressing) for each format, plus the reverse pair (`Encoder` over `Read`), with limits. | 0.5 |
| FR-16 | Checksums: CRC-32 (ISO-HDLC, as gzip) and Adler-32, one-shot and streaming, with `combine`. | 0.2 |
| FR-17 | Brotli decode (RFC 7932), then encode. | 1.1, 1.2 |

### 4.2 Non-functional

| Id | Requirement | Check |
|---|---|---|
| NFR-1 | Zero third-party runtime dependencies, with every feature combination. | `cargo tree -e normal` in CI |
| NFR-2 | `#![forbid(unsafe_code)]`. | crate attribute |
| NFR-3 | `no_std` with `alloc` for the codecs; `std` (default) adds `std::io` adapters and `std::error::Error` is via `core::error::Error`. | CI builds `--no-default-features` for `thumbv7em-none-eabi` |
| NFR-4 | Memory per engine fixed at construction and stated per configuration (section 5.6); no allocation after construction on the slice API; proven with a counting allocator. | `tests/alloc.rs` |
| NFR-5 | Bounded work per call: every call does at most `O(input consumed + output produced + 2^window_bits)` work; no quadratic path on any input, including one-byte output buffers. | property test on work counters, benchmark with 1-byte buffers |
| NFR-6 | No panics on any input, any split, any buffer size. | fuzz, proptest |
| NFR-7 | Deterministic output: the same input, level, window, and flush sequence give the same bytes on every platform and pointer width. | CI compares a digest of the compressed corpus on every OS |
| NFR-8 | Speed and ratio targets of section 9. | `bench-compare/` |
| NFR-9 | `Send` and `Sync` for every engine (no interior mutability, no globals); tables built at compile time (`const`). | compile-time assertions |

### 4.3 Enterprise and distributed

Stateless row of standard 2.16: a codec. Deterministic, no process-local assumptions, safe to run the same version on many nodes; a stream compressed on one node decompresses on any other. Observability: every engine reports `total_in()` and `total_out()`; every error has a stable `code()` string (`"compress.inflate.distance_too_far"`) for metrics; the cost of the counters is two additions per call. Operability: every limit configurable with a safe default (section 6.2); behaviour at a limit is a clean error, after which the engine refuses further input until `reset()`.

---

## 5. Design and architecture

### 5.1 Modules

| Module | Visibility | Contents |
|---|---|---|
| `lib.rs` | public | `VERSION`, re-exports, Tier 1 docs |
| `error.rs` | public | `Error`, `ErrorKind` |
| `limits.rs` | public | `Limits` |
| `level.rs` | public | `Level` |
| `checksum/` | public | `crc32`, `Crc32`, `adler32`, `Adler32` |
| `bits.rs` | private | `BitReader` (64-bit buffer, refill by whole bytes), `BitWriter` |
| `huffman/` | private | decode table builder, length-limited code builder for the encoder |
| `inflate/` | private, re-exported through `deflate` | state machine, fast loop, window ring |
| `deflate/` | public | `Inflater`, `Deflater`, `Flush`, Tier 1 functions; compressor internals in private submodules (`matcher`, `block`, `lazy`, `fast`) |
| `message.rs` | public | RFC 7692 `Compressor`, `Decompressor`, `MessageConfig` |
| `zlib.rs`, `gzip.rs` | public | framing engines, Tier 1 functions, `GzipHeader` |
| `format.rs` | public | `Format`, `Decoder`, `Encoder` (format-generic engines) |
| `io.rs` | public, feature `std` | `Reader`, `Writer` adapters |

### 5.2 Inflater

**Bit reader.** A `u64` bit buffer filled whole bytes at a time from the input slice. In the fast loop the refill reads eight bytes with `u64::from_le_bytes` when at least eight remain (branch-free refill as in libdeflate); the slow path refills byte by byte. Bytes in the bit buffer are part of the engine state, so a call that stops early keeps them; `total_in` counts bytes taken into the buffer, and at stream end the unused whole bytes are returned to the caller (`Progress::consumed` excludes them) so trailing framing (zlib Adler-32, gzip trailer, the next gzip member) is read from the caller's slice.

**Tables.** Litlen decode table with an 11-bit primary index and subtables (at most 2342 entries), distance table with an 8-bit primary (at most 402), code-length table 7-bit (128). Each entry is a `u32` carrying the symbol or the base value, the number of bits to consume (code plus extra bits, so one shift consumes both), the extra-bit count, and a kind tag (literal, length, end of block, subtable link, invalid). The fixed-Huffman tables are built at compile time by a `const fn`. Built tables are checked for completeness; an incomplete code is accepted only where RFC 1951 allows it (a single distance code, or none when the block has only literals), and a lookup that lands on an unused entry is `InvalidCode`.

**Output and history.** The decoder writes straight into the caller's output slice (no internal staging copy). History for back-references is the output already written in this call, and, for distances that reach further back, a ring buffer of `2^window_bits` bytes kept by the engine. The ring is updated once per call with the last `min(produced, window)` bytes, so a sequence of tiny calls costs `O(produced)` each, never `O(window)` (NFR-5). For `decompress_vec` the vector itself is the history; the ring is only touched when the call ends with the stream unfinished and the caller may reuse or clear the vector (context takeover or streaming). A caller that decodes a whole message into one vector and resets after it (no takeover) never touches the ring and it is never allocated (ws-net idea of 0.5: saves 32 KiB per connection).

**Fast loop.** Entered when at least 16 input bytes and 274 output bytes (one literal plus a maximum match, rounded) remain: decode a literal or a length and distance with no state-machine dispatch, copy the match with an overlapping-copy routine that uses 8-byte chunk copies when `distance >= 8` and a pattern-expansion loop for short distances (`distance == 1` is a fill). When the margins are not there, the slow path decodes one symbol at a time and saves the exact state (pending length and distance, bits held) when input or output runs out.

**State machine.** `BlockHeader`, `StoredLength`, `Stored { remaining }`, `DynamicHeader { stage }`, `CodeLengths { index }`, `Data`, `Match { length, distance }` (output full mid-match), `Done`, `Failed`. A failed engine returns the same error from every call until `reset()` (sticky errors).

**Limits.** Before writing each chunk (a literal run, a match, a stored segment), the decoder trims its output slice so that the total never passes `max_output`, and if output stops because of the cap while the stream is unfinished, the call returns `Error::OutputLimit`. The ratio check (`total_out > ratio_floor && total_out > max_ratio * total_in`) runs per chunk; cost one multiply and two compares per chunk [est. under 1% at 64 KiB chunks].

### 5.3 Deflater

**Window.** A buffer of `2 * 2^window_bits` bytes plus lookahead (258 + 4); input is copied into it (one copy per byte, needed for match finding across calls), and it slides by `2^window_bits` when full (one `copy_within` per window of input, so half a copy per byte amortised).

**Match finding.** Hash of 3 bytes (levels 1 to 3: 4 bytes, faster and better for text, as zlib-ng's `quick`), multiplicative hash, `head: [u16; 2^hash_bits]` and `prev: [u16; 2^window_bits]` chains for levels 2 to 9. Positions are stored relative to the window base and adjusted on slide.

| Level | Strategy | Good | Lazy (greedy: insert limit) | Nice | Chain |
|---|---|---|---|---|---|
| 0 | stored blocks | n/a | n/a | n/a | n/a |
| 1 | greedy, one probe, 4,096-entry hash, sparse search through long misses (D-040) | 4 | 4 | 8 | 1 |
| 2 | greedy | 4 | 5 | 16 | 8 |
| 3 | greedy | 4 | 6 | 32 | 32 |
| 4 | lazy | 4 | 4 | 16 | 16 |
| 5 | lazy | 8 | 16 | 32 | 32 |
| 6 (default) | lazy | 8 | 16 | 128 | 128 |
| 7 | lazy | 8 | 32 | 128 | 256 |
| 8 | lazy | 32 | 128 | 258 | 1024 |
| 9 | lazy | 32 | 258 | 258 | 4096 |

The table starts from zlib's configuration (so ratios are comparable) and is tuned in 0.6 against the corpus; tuning changes are decisions.

**Blocks.** Symbols go to a buffer of 16,384 entries (litlen symbol plus distance, 4 bytes each). When it fills, or on flush, the block coder computes the exact cost of stored, fixed, and dynamic encodings and writes the cheapest. Dynamic codes are length-limited to 15 (litlen, distance) and 7 (code lengths) with a package-merge-free heuristic: build Huffman lengths with a two-queue algorithm on sorted frequencies, then repair overflow by the zlib method (move leaves down, recount). Deterministic tie-breaking (symbol order) for NFR-7.

**Output.** Bits go to a pending buffer (64 KiB plus one maximum block) the caller drains through `compress(input, output, flush)`. The engine never holds more than one block of pending output plus the symbol buffer; when the caller's output is full it stops taking input (back-pressure).

### 5.4 Per-message (RFC 7692)

`message::Compressor` wraps a `Deflater` with `Flush::Sync` per message, removes the final `00 00 FF FF`, and either keeps history (takeover) or calls `reset()` (no takeover). If a message compresses to more bytes than it holds (rare, incompressible), the caller may send it uncompressed; `compress_message` returns the compressed length so the caller can decide (ws-net's `min_size` and per-message opt-out live in ws-net). An empty message compresses to `00` (RFC 7692 section 7.2.3.6).

`message::Decompressor` wraps an `Inflater`; `decompress_message(payload, out: &mut Vec<u8>, budget)` feeds the payload and then the four tail bytes, writes into `out` with the budget checked per chunk (ws-net FR-41), and accepts a stream that ends with `BFINAL` (then calls `next_stream()` with takeover, `reset()` without). Data after a final block in the same message is `TrailingData` (ws-net 1007).

### 5.5 zlib and gzip

Framing engines wrap the raw engines and parse or write the header and trailer through small state machines that also resume at any byte. The gzip header parser has per-field caps (`Limits::gzip_header`, default 64 KiB total for FEXTRA, FNAME, FCOMMENT together) and stores FNAME and FCOMMENT only if the caller asks (`with_keep_header(true)`, bounded by the same cap). Multi-member gzip continues after each trailer while input starts with the gzip magic; `max_members` (default 1024) bounds the loop on hostile input that is all empty members.

### 5.6 Memory per engine (estimates until measured in 0.6)

| Engine | Window bits 15 | Window bits 9 |
|---|---|---|
| `Inflater` (slice API, ring allocated) | 32 KiB ring + 11 KiB tables + 0.2 KiB state = about 43 KiB | 0.5 KiB ring + 11 KiB + 0.2 KiB = about 12 KiB |
| `Inflater` used through `decompress_vec` with reset per message | 11 KiB tables, no ring | same |
| `Deflater` level 1 | 64 KiB window + 16 KiB head (13 hash bits) + 64 KiB symbols + 64 KiB pending = about 208 KiB | 1 KiB + 16 KiB + 64 KiB + 64 KiB = about 145 KiB |
| `Deflater` level 6 | + 64 KiB `prev`: about 272 KiB | + 1 KiB: about 146 KiB |

For comparison zlib's deflate at `windowBits 15, memLevel 8` is about 256 KiB and its inflate 7 KiB plus a 32 KiB window. `MemoryConfig` (Tier 3) lets ws-net shrink the symbol and pending buffers (`with_block_symbols(4096)` halves the 145 KiB case) at a small ratio cost, measured in 0.6.

---

## 6. API specification

Ids: `ROOT-`, `ERR-`, `LIM-`, `LVL-`, `CK-` (checksums), `DF-` (deflate), `MSG-` (message), `ZL-`, `GZ-`, `FMT-`, `IO-`, `BR-` (brotli). Each row: grade, tier, version.

### 6.1 Root

| Id | Item | Grade | Tier | Ver |
|---|---|---|---|---|
| ROOT-01 | `pub const VERSION: &str` | P1 | 1 | 0.1 |
| ROOT-02 | `Error`, `ErrorKind`, `Limits`, `Level` re-exported at the root | P0 | 1 | 0.2 |

### 6.2 Limits and levels

```rust
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct Limits { /* private fields */ }
```

| Id | Item | Default | Grade | Tier | Ver |
|---|---|---|---|---|---|
| LIM-01 | `Limits::new()`, `Default` | `max_output` 64 MiB, ratio off, `ratio_floor` 1 MiB, gzip header 64 KiB, `max_members` 1024 | P0 | 2 | 0.3 |
| LIM-02 | `with_max_output(u64)`, `max_output()` | | P0 | 2 | 0.3 |
| LIM-03 | `with_max_ratio(u32)` (0 = off), `with_ratio_floor(u64)` | | P0 | 2 | 0.3 |
| LIM-04 | `Limits::unlimited()` (named so its use is visible in review) | | P1 | 2 | 0.3 |
| LIM-05 | `with_max_header(usize)`, `with_max_members(u32)` (gzip) | | P1 | 2 | 0.5 |
| LVL-01 | `Level`: `Level::NONE` (0), `FAST` (1), `DEFAULT` (6), `BEST` (9), `Level::new(u8) -> Result<Level>`, `Level::get()` | | P0 | 2 | 0.4 |

Why the ratio cap is off by default: DEFLATE's largest possible ratio is about 1032:1, legitimate data (zero-filled images, repeated logs) reaches hundreds to one, and the absolute cap is what bounds memory. The ratio cap exists for callers that know their data (an API gateway that expects JSON at under 50:1). Cost of both checks: one compare (absolute) and one multiply-compare (ratio) per output chunk.

### 6.3 Errors

```rust
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Error { kind: ErrorKind, offset: u64 }   // offset: input byte where it was found

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum ErrorKind { /* below */ }
```

| Id | Kind | Meaning, and what to do | Ver |
|---|---|---|---|
| ERR-01 | `InvalidBlockType` | block type 3; the data is not DEFLATE or is corrupt | 0.3 |
| ERR-02 | `InvalidStoredLength` | LEN and NLEN disagree | 0.3 |
| ERR-03 | `InvalidCodeLengths` | a code-length table that over-subscribes, is incomplete where not allowed, repeats with no previous length, or runs past the alphabet | 0.3 |
| ERR-04 | `InvalidSymbol` | litlen 286/287 or distance 30/31, or a code that maps to no symbol | 0.3 |
| ERR-05 | `DistanceTooFar` | a back-reference before the start of the data (and dictionary) or beyond the window | 0.3 |
| ERR-06 | `UnexpectedEnd` | input ended inside the stream (`finish` was called); the data is truncated | 0.3 |
| ERR-07 | `OutputLimit` | `max_output` reached; raise the limit only if the source is trusted | 0.3 |
| ERR-08 | `RatioLimit` | `max_ratio` exceeded; likely a decompression bomb | 0.3 |
| ERR-09 | `TrailingData` | bytes after the end of the stream; strip them or allow them | 0.3 |
| ERR-10 | `InvalidWindowBits`, `InvalidLevel` | a configuration value out of range (8 to 15, 0 to 9) | 0.3 / 0.4 |
| ERR-11 | `InvalidHeader` | zlib or gzip header check failed | 0.5 |
| ERR-12 | `ChecksumMismatch` | Adler-32, CRC-32, or header CRC differs; the data is corrupt | 0.5 |
| ERR-13 | `LengthMismatch` | gzip ISIZE differs | 0.5 |
| ERR-14 | `HeaderTooLarge`, `TooManyMembers` | gzip limits | 0.5 |
| ERR-15 | `DictionaryRequired`, `WrongDictionary` | zlib FDICT; call `set_dictionary` with the right one | 0.5 |
| ERR-16 | `Error::kind()`, `offset()`, `code() -> &'static str`, `Display` ("say what to do"), `core::error::Error` | | 0.3 |

### 6.4 Checksums

| Id | Item | Cost target | Grade | Tier | Ver |
|---|---|---|---|---|---|
| CK-01 | `checksum::crc32(&[u8]) -> u32` | slicing-by-16, at least 4 GB/s on a 2020s x86_64 core [target] | P1 | 1 | 0.2 |
| CK-02 | `Crc32::new()`, `update(&mut self, &[u8])`, `finish(&self) -> u32`, `Default` | same | P1 | 3 | 0.2 |
| CK-03 | `checksum::adler32(&[u8]) -> u32`, `Adler32` | at least 8 GB/s [target], 5552-byte blocks with deferred modulo | P1 | 1/3 | 0.2 |
| CK-04 | `crc32_combine(a, b, len_b)`, `adler32_combine` | O(log n) | P3 | 3 | 0.2 |

### 6.5 Raw DEFLATE (`compress::deflate`)

Tier 1 and 2:

| Id | Item | Grade | Tier | Ver |
|---|---|---|---|---|
| DF-01 | `deflate::decompress(&[u8]) -> Result<Vec<u8>, Error>` (default limits) | P0 | 1 | 0.3 |
| DF-02 | `deflate::decompress_with(&[u8], Limits) -> Result<Vec<u8>, Error>` | P0 | 2 | 0.3 |
| DF-03 | `deflate::compress(&[u8]) -> Vec<u8>` (level 6) | P0 | 1 | 0.4 |
| DF-04 | `deflate::compress_with(&[u8], Level) -> Vec<u8>` | P0 | 2 | 0.4 |

Tier 3, `Inflater`:

```rust
pub struct Inflater { /* ... */ }

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct Progress { pub consumed: usize, pub produced: usize, pub status: Status }

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum Status { NeedsInput, NeedsOutput, Done }
```

| Id | Item | Behaviour | Grade | Ver |
|---|---|---|---|---|
| DF-10 | `Inflater::new()`, `Default` | window bits 15, default limits | P0 | 0.3 |
| DF-11 | `with_window_bits(u8) -> Result<Self>`, `with_limits(Limits) -> Self` | | P0 | 0.3 |
| DF-12 | `decompress(&mut self, input: &[u8], output: &mut [u8]) -> Result<Progress, Error>` | push input, pull output; `NeedsInput` when all input is used and the stream is unfinished; `NeedsOutput` when output is full; `Done` at the end of the final block | P0 | 0.3 |
| DF-13 | `finish(&mut self, input, output) -> Result<Progress, Error>` | as DF-12 but this is the last input: an unfinished stream is `UnexpectedEnd` | P0 | 0.3 |
| DF-14 | `decompress_vec(&mut self, input: &[u8], out: &mut Vec<u8>) -> Result<Progress, Error>` | appends; reserves in chunks of at most 64 KiB and never past the cap | P0 | 0.3 |
| DF-15 | `is_done()`, `total_in()`, `total_out()`, `window_bits()`, `limits()` | | P1 | 0.3 |
| DF-16 | `reset(&mut self)` | forget history and state, keep allocations | P0 | 0.3 |
| DF-17 | `next_stream(&mut self)` | after `Done`: start a new stream keeping the window (RFC 7692 takeover after `BFINAL`) | P0 | 0.4 |
| DF-18 | `set_dictionary(&mut self, &[u8])` | preset history (the last `2^bits` bytes) | P1 | 0.4 |
| DF-19 | `memory_in_use(&self) -> usize` | heap bytes held | P2 | 0.4 |

Tier 3, `Deflater`:

| Id | Item | Behaviour | Grade | Ver |
|---|---|---|---|---|
| DF-30 | `Deflater::new(Level)`, `Default` (level 6, bits 15) | | P0 | 0.4 |
| DF-31 | `with_window_bits(u8) -> Result<Self>` | 8 to 15 | P0 | 0.4 |
| DF-32 | `compress(&mut self, input, output, Flush) -> Result<Progress, Error>` | takes as much input as fits, writes pending output; with a flush, `Done` means the flush point is fully written | P0 | 0.4 |
| DF-33 | `compress_vec(&mut self, input, &mut Vec<u8>, Flush) -> Result<Progress, Error>` | appends until the flush point is written | P0 | 0.4 |
| DF-34 | `Flush::{None, Sync, Full, Finish}` (`#[non_exhaustive]`) | | P0 | 0.4 |
| DF-35 | `reset`, `set_dictionary`, `total_in`, `total_out`, `memory_in_use` | | P1 | 0.4 |
| DF-36 | `with_level(Level)` mid-stream (applies from the next block) | | P2 | 0.4 |
| DF-37 | `MemoryConfig` (block symbols, hash bits) | section 5.6 | P2 | 0.6 |

### 6.6 Per-message (`compress::message`, RFC 7692)

| Id | Item | Behaviour | Grade | Ver |
|---|---|---|---|---|
| MSG-01 | `Compressor::new(MessageConfig)` | | P0 | 0.4 |
| MSG-02 | `compress_message(&mut self, msg: &[u8], out: &mut Vec<u8>) -> Result<usize, Error>` | appends the payload without the tail, returns its length | P0 | 0.4 |
| MSG-03 | `Decompressor::new(MessageConfig)` | | P0 | 0.4 |
| MSG-04 | `decompress_message(&mut self, payload: &[u8], out: &mut Vec<u8>, max: usize) -> Result<usize, Error>` | appends; output budget per chunk; returns bytes appended | P0 | 0.4 |
| MSG-05 | `MessageConfig::new()` with `with_window_bits(u8)`, `with_context_takeover(bool)`, `with_level(Level)` | defaults: bits 15, no takeover, level 1 | P0 | 0.4 |
| MSG-06 | `reset()`, `memory_in_use()` on both | | P1 | 0.4 |
| MSG-07 | Streaming per-message decode for fragmented messages: `decompress_fragment(&mut self, fragment, last: bool, out, max)` | ws-net decodes fragments as they arrive | P1 | 0.4 |

### 6.7 zlib (`compress::zlib`) and gzip (`compress::gzip`)

| Id | Item | Grade | Tier | Ver |
|---|---|---|---|---|
| ZL-01 | `zlib::compress`, `compress_with`, `decompress`, `decompress_with` | P0 | 1/2 | 0.5 |
| ZL-02 | `zlib::Decoder`, `zlib::Encoder`: same streaming shape as `Inflater`/`Deflater`, plus `set_dictionary` / `dictionary_id()` | P0 | 3 | 0.5 |
| GZ-01 | `gzip::compress`, `compress_with`, `decompress`, `decompress_with` | P0 | 1/2 | 0.5 |
| GZ-02 | `gzip::Decoder` (multi-member by default, `with_single_member()`, `with_trailing_data(bool)`), `gzip::Encoder` (`with_header(GzipHeader)`) | P0 | 3 | 0.5 |
| GZ-03 | `GzipHeader { mtime, os, extra_flags, text, name, comment, extra }` read and write (bounded) | P2 | 3 | 0.5 |
| FMT-01 | `Format::{Raw, Zlib, Gzip}`, `Format::detect(&[u8]) -> Option<Format>` | P1 | 2 | 0.5 |
| FMT-02 | `Decoder::new(Format)`, `Encoder::new(Format, Level)`: one type for code that picks the format at run time (http-net content codings) | P1 | 3 | 0.5 |
| FMT-03 | `compress_lib::decompress(&[u8])` auto-detecting gzip or zlib | P2 | 1 | 0.5 |

### 6.8 std::io (feature `std`, default)

| Id | Item | Grade | Ver |
|---|---|---|---|
| IO-01 | `io::Reader<R: Read>` (decompress while reading), `Reader::new(inner, Format)`, `with_limits` | P1 | 0.5 |
| IO-02 | `io::Writer<W: Write>` (compress while writing), `finish() -> io::Result<W>`, `flush()` = sync flush | P1 | 0.5 |
| IO-03 | `io::Encoder<R: Read>` and `io::DecodeWriter<W: Write>` (the reverse pairs, as flate2's `read::GzEncoder` and `write::GzDecoder`) | P2 | 0.5 |

### 6.9 Brotli (after 1.0)

| Id | Item | Grade | Ver |
|---|---|---|---|
| BR-01 | `brotli::decompress`, `Decoder` streaming with limits, built-in dictionary as a `const` table | P2 | 1.1 |
| BR-02 | `brotli::compress`, `Encoder`, quality 0 to 11 | P2 | 1.2 |

### 6.10 Features

| Feature | Default | Adds |
|---|---|---|
| `std` | yes | `io` adapters, `std::error::Error` through `core` |
| `alloc` | yes (via `std`) | `Vec` APIs; without it only the slice API and checksums (P3, if the table can be caller-provided; otherwise `alloc` is required and documented) |

---

## 7. Parity matrix

| Capability | compress-lib (target) | miniz_oxide 0.8 | zlib-rs 0.5 | flate2 1.x | zlib 1.3 (C) |
|---|---|---|---|---|---|
| Raw / zlib / gzip | yes / yes / yes | raw, zlib; gzip via flate2 | raw, zlib, gzip | all (backends) | all |
| Streaming, caller buffers | yes, sans-IO | yes (`inflate::stream`, core) | yes (z_stream API) | `Read`/`Write` and `Compress`/`Decompress` | yes |
| Window bits 8 for deflate | yes, real 256-byte window | inflate only via full window | rounds 8 to 9 (zlib behaviour) | backend | rounds 8 to 9 |
| Output caps during decode | absolute and ratio, per chunk | no (caller-managed) | no | no | no |
| Sync, full flush | yes | yes | yes | yes | yes |
| RFC 7692 helpers | yes | no | no | no | no |
| Multi-member gzip | yes, bounded | n/a | yes | `MultiGzDecoder` | yes |
| Bounded gzip header | yes, configurable | n/a | `gz_header` caller buffers | `GzHeader` unbounded `Vec` | caller buffers |
| `no_std` | yes (`alloc`) | yes | partial | no | n/a |
| `unsafe` | forbidden | `forbid` in core | used (SIMD, FFI surface) | uses backends | C |
| Runtime dependencies | 0 | `adler2` | 0 | `crc32fast`, backend | n/a |
| Brotli | 1.1 | no | no | no | no |

---

## 8. Security

| Id | Threat | Defence | Attack test | Cost | Ver |
|---|---|---|---|---|---|
| T-1 | Decompression bomb: 1 KiB expands to gigabytes (undici CVE-2026-1526, libsoup CVE-2026-15709) | absolute output cap checked per chunk before writing; ratio cap optional | 64 MiB of zeros compressed to 64 KiB fails with `OutputLimit` at exactly the cap; peak memory at most 1.5 times the cap plus the tables (counting allocator, D-021) | 1 compare per chunk | 0.3 |
| T-2 | Ratio bomb hidden under the absolute cap across many requests | ratio cap after a floor | stream at 1000:1 fails at `ratio_floor` with `max_ratio 100` | 1 multiply per chunk | 0.3 |
| T-3 | Malformed Huffman tables (over-subscribed, incomplete, huge repeat runs) causing out-of-bounds reads or loops | table builder validates Kraft sums, counts, and repeat bounds; entries tagged invalid | crafted tables for each case return the matching error; fuzz | none at decode time | 0.3 |
| T-4 | Distance beyond output or window (reads uninitialised or stale memory in C decoders) | checked against produced plus dictionary and the window | distances 1 past each bound fail with `DistanceTooFar` | 1 compare per match | 0.3 |
| T-5 | Quadratic work from tiny output buffers or byte-by-byte input | ring updated `O(produced)`; no per-call window copy | 1 MiB stream decoded with 1-byte output buffers in under 50x the one-shot time | none | 0.3 |
| T-6 | Infinite loop on empty stored blocks or empty gzip members | every loop iteration consumes input or produces output; `max_members` | 10 MB of empty stored blocks terminates linearly; 1 MiB of empty gzip members fails at `max_members` | 1 counter | 0.3/0.5 |
| T-7 | gzip header memory exhaustion (unbounded FNAME/FCOMMENT/FEXTRA) | per-header cap; fields kept only when asked | 1 MiB FNAME fails with `HeaderTooLarge`; with keeping off, no allocation for header fields | 1 counter per header byte | 0.5 |
| T-8 | Checksum stripping or forgery to pass corrupted data | Adler-32, CRC-32, ISIZE, FHCRC always verified; no "ignore checksum" option | flipped trailer bytes fail | checksum cost, measured in 0.6 (about 5% of inflate [est.]) | 0.5 |
| T-9 | CRIME/BREACH style length side channels when secrets and attacker data share a compression context | documentation: compress secrets separately or not at all; `message` defaults to no context takeover; `Flush::Full` resets history mid-stream | test shows two messages without takeover compress independently | ratio loss, measured | 0.4 |
| T-10 | Per-connection memory exhaustion (many engines) | memory fixed per configuration and reported by `memory_in_use`; small windows real (bits 8 is 256 bytes) | `memory_in_use` matches section 5.6 for every configuration | none | 0.4 |
| T-11 | Panic on hostile input taking down a server thread | no panic paths; fuzz; `forbid(unsafe_code)` | fuzz 24 h without a crash before 0.9; proptest "never panics" | none | 0.7 |
| T-12 | Truncated stream treated as complete (silent data loss) | `finish` reports `UnexpectedEnd`; Tier 1 functions always finish | every prefix of a valid stream fails with `UnexpectedEnd` under `finish` | none | 0.3 |
| T-13 | Trailing data smuggling (bytes after the stream treated as something else) | `TrailingData` by default in Tier 1 and framing; streaming returns `consumed` exactly | valid stream plus 1 byte fails in Tier 1 | none | 0.3 |
| T-14 | Encoder resource abuse (attacker-supplied data that triggers worst-case chain walks at level 9) | chain limits per level; level 9 bounded at 4096 probes per position | adversarial input (all same hash, distinct bytes) at level 9 stays within 20x level 6 time | none | 0.4 |

New defence idea per version is recorded in `dev/PLAN.md` (ideas section); the first (0.1): the inflater's ratio check counts input taken into the bit buffer, so an attacker cannot lower the measured input by keeping bytes pending; checked by a test feeding one byte at a time.

---

## 9. Performance

### 9.1 Targets (measured in 0.6, single core, throughput of uncompressed bytes)

| Benchmark | Target |
|---|---|
| inflate, text corpus, level 6 data | at least 1.2x miniz_oxide; parity with zlib-rs [stretch] |
| inflate, binary corpus | at least 1.2x miniz_oxide |
| deflate level 1 | at least 1.2x miniz_oxide level 1 at equal or better ratio |
| deflate level 6 | at least 1.2x miniz_oxide level 6 at ratio within 1% |
| deflate level 9 | ratio within 0.5% of zlib level 9; speed at least miniz_oxide's |
| CRC-32 | at least `crc32fast`'s software path |
| Adler-32 | at least `adler2` |
| permessage-deflate small messages (100 B to 4 KiB JSON), no takeover | per-message cost below miniz_oxide's with a reset per message |

### 9.2 Method

`bench-compare/` (not published, excluded from the package) holds criterion benches against `miniz_oxide`, `zlib-rs` (`libz-rs-sys`), `flate2` with `miniz_oxide`, `zlib-rs`, and `zlib` (C, `libz-sys`) backends, and zlib-ng where a C toolchain exists (Linux runner). Corpora are generated deterministically in the bench (text from a seeded word model, JSON records, HTML-like markup, binary structs, random bytes, zeros) plus the repository's own files, so no download is needed and numbers are reproducible. Ratios are reported with speeds. A hosted-runner workflow publishes numbers for Linux, macOS, and Windows into `docs/PERFORMANCE.md`; the local machine is shared and loaded, so its numbers are labelled as such.

---

## 10. Testing plan

- **Unit tests** at the bottom of each file, every `Ok` and `Err` path; names `test_<subject>_<condition>_<expected>`.
- **Vectors**: RFC 1951 hand-built blocks for each error; RFC 7692 section 7.2.3 examples (compressed "Hello" with and without takeover `f2 48 cd c9 c9 07 00`, the reused-window second message `f2 00 11 00 00`, the empty block, the `BFINAL` variant, two stored blocks); zlib and gzip from RFC examples and files produced by GNU gzip and zlib (checked-in small files).
- **Differential**: inflate vs `miniz_oxide` on random valid streams (produced by miniz_oxide at all levels and by compress-lib) and on random bytes (both agree on accept or reject, and on output when both accept); deflate output decoded by miniz_oxide.
- **Round trips** (proptest): every level, window bits 8 to 15, every flush pattern, random split of input and random output buffer sizes; output identical to the original.
- **Split invariance**: input split at every byte boundary for vectors and small corpora; output buffer of every size 1 to 300.
- **Edge cases**:
  - E-1 empty input to `decompress` (`UnexpectedEnd`); E-2 empty input to `compress` (valid empty stream, `03 00` raw); E-3 one byte; E-4 exactly 65,535 and 65,536 bytes stored; E-5 match length 258 and 3; E-6 distance 1, 32,768, 32,769 (error); E-7 window bits 8 with distance 256 and 257; E-8 output exactly at the cap and one byte more; E-9 ratio exactly at the cap; E-10 block with only end-of-block; E-11 dynamic block with one distance code and with none; E-12 code-length repeat 16 as the first entry (error); E-13 repeat running past 286+30 (error); E-14 all litlen lengths zero except end-of-block; E-15 litlen 286 used (error); E-16 truncated at every byte of a valid stream; E-17 bits after the final block (ignored inside the last byte, `TrailingData` for whole bytes in Tier 1); E-18 stored block LEN=0 (sync flush); E-19 thousands of empty stored blocks; E-20 gzip with every header flag; E-21 gzip FHCRC wrong; E-22 gzip reserved flag bits set (error); E-23 gzip two members and 1,025 members; E-24 gzip ISIZE wrong; E-25 zlib CINFO 8 (error), FCHECK wrong, FDICT without dictionary; E-26 `next_stream` before `Done` (error kind `InvalidState`); E-27 sync flush with no input (`00 00 FF FF` only); E-28 full flush then a match that would reach across it (never emitted); E-29 per-message empty message; E-30 per-message message whose compressed form is larger than the input.
- **Allocation**: counting allocator proves the slice APIs allocate nothing after construction, and `decompress_vec` reserves at most one chunk past the cap.
- **Fuzz** (cargo-fuzz, corpus outside the repository): `inflate_raw` (differential vs miniz_oxide), `inflate_split` (random splits equal one call), `roundtrip` (random data, level, bits, flush plan), `zlib_decode`, `gzip_decode`, `message` (ws-net flow). 24 hours on the inflater targets before 0.9; 20 s per target on every CI push; 10 minutes nightly.
- **Miri**: the bit reader and table builder (cheap), on every push.
- **Determinism**: digest of compressed corpus compared across CI OSes.
- **Soak** (0.7): 4 h of random round trips with a memory check.

---

## 11. Documentation deliverables

README (template, Tier 1 first), rustdoc for every public item with runnable examples, `docs/API.md` (metrics-lib format), `docs/ARCHITECTURE.md`, `docs/PERFORMANCE.md`, `docs/SECURITY.md` (threat model, limits, CRIME/BREACH guidance), `docs/PLATFORM-NOTES.md`, `docs/STABILITY-1.0.md`, `CHANGELOG.md`, `docs/release/vX.Y.Z.md`, examples (`gzip_file`, `streaming`, `websocket_message`, `http_body`).

---

## 12. Version roadmap

| Version | Milestone | Contents | Exit |
|---|---|---|---|
| 0.1.0 | Scaffold | repository, manifest, CI, plan, roadmap, directives; name claimed | CI green; published |
| 0.2.0 | Design lock | `docs/API.md` with the whole 1.0 API and versions; checksums (CK-01 to CK-04) as the first real code; error and limit types | API written and self-approved under James's standing instruction (D-010); checksum vectors and benches |
| 0.3.0 | Core: inflate | raw DEFLATE inflater, streaming, limits, window bits, Tier 1 `deflate::decompress`; differential tests and the first fuzz targets | every E-n for inflate; differential clean; fuzz smoke clean |
| 0.4.0 | Extended: deflate and RFC 7692 | deflater levels 0 to 9, flush modes, window bits 8 to 15, dictionaries, `message` module | round trips; miniz_oxide decodes everything; RFC 7692 vectors; ws-net can adopt |
| 0.5.0 | Feature complete | zlib, gzip, `Format`, `io` adapters, auto-detect | every FR to FR-16 met |
| 0.6.0 | Optimisation | `bench-compare/`, tuning, hosted numbers, `docs/PERFORMANCE.md` | section 9 targets met or the gap recorded with its reason |
| 0.7.0 | Hardening | fuzz targets complete, 24 h inflater fuzz run, attack test per threat, soak, `docs/SECURITY.md` | no crash, no leak, every T-n test passes |
| 0.8.0 | Alpha | consumer runs: ws-net permessage-deflate on a branch with Autobahn 12 and 13, http-net body decode example, serve-net response encode example | consumers green; issues fixed |
| 0.9.x | Beta and polish | API audit (names, `must_use`, `non_exhaustive`), docs audit, `docs/STABILITY-1.0.md` draft, MSRV policy, deprecations none, release candidates | audit recorded; nothing open but the 1.0 decision |
| 1.0.0 | Stable | James's explicit go after the audit | |
| 1.1 | Brotli decode | BR-01 | |
| 1.2 | Brotli encode | BR-02 | |

No series is skipped. P0 items never move; a P1 or P2 move is recorded with its reason in `dev/ROADMAP.md` and the decision log.

---

## 13. Definition of done

Every P0 and P1 item and every P2 item not moved with a reason is implemented, documented with examples, and tested; every threat has its attack test; every edge case has a test; the section 9 targets are met or the gap is published; fuzzing has run 24 h on the inflater targets without a finding; `docs/STABILITY-1.0.md` records the audit; James has said go.

---

## 14. Open decisions

| Id | Question | Recommendation (default) |
|---|---|---|
| OD-1 | `forbid(unsafe_code)` against the speed of zlib-rs, which uses SIMD and `unsafe` | Keep `forbid`. Reach speed through 64-bit bit buffers, multi-entry tables, and chunked copies. Publish the gap; revisit only with a measured case and a recorded decision. |
| OD-2 | Default `max_output` for Tier 1 decode | 64 MiB: large enough for normal HTTP bodies and WebSocket messages, small enough that a bomb cannot take a small server down. Streaming engines use the same default; long-running streams set `Limits::unlimited()` or their own cap explicitly. |
| OD-3 | Ratio cap default | Off (section 6.2). |
| OD-4 | Level numbering | zlib's 0 to 9, so consumers and operators keep their intuition; 10 to 12 reserved for a possible optimal parser (P3). |
| OD-5 | Default level for `message` | 1: per-message traffic is latency-sensitive and small; matches ws-net A-270. |
| OD-6 | Brotli placement | In this crate after 1.0 (1.1 decode, 1.2 encode), behind a non-default `brotli` feature so its 120 KiB dictionary costs nothing for users who do not want it. |
| OD-7 | HTTP `deflate` coding | It is zlib framing (RFC 9110); `Format::Zlib` is what http-net uses; documentation says so, and a decoder option to accept raw DEFLATE for broken peers is P3. |

---

---

## 15. Ideas parking lot

One new defence idea per version: hypothesis, method, expected cost.

| Version | Idea | Method | Expected cost |
|---|---|---|---|
| 0.6 | Speed work is where safety checks quietly disappear. Every 0.6 change kept `forbid(unsafe_code)` and the bounds checks, and the match decisions at levels 2 to 9 were required to stay those of 0.5, so a speed change could not hide a ratio change. | Compressed sizes at levels 2 to 9 were compared before and after each change; the full test and fuzz suites pass unchanged. | none at run time |
| 0.5 | A multi-member gzip stream can carry many members that each stay under the output cap; capping per member would let the sum exceed it. The cap is shared: each new member's inflater gets the cap minus what earlier members produced. | `test_framing_gzip_members_and_cap`: two members of 600 bytes fail a 1,000-byte cap and pass a 1,200-byte one. | one subtraction per member (D-034) |
| 0.4 | A WebSocket decoder that tolerates any bytes after a final block lets a peer smuggle data the application never sees but intermediaries may log or forward; RFC 7692's own example has one zero byte there. Accept only zero padding after `BFINAL`, refuse anything else. | `test_message_trailing_after_final_block_rejected`: zero padding accepted, a nonzero byte is `TrailingData`; the `message` fuzz target. | one scan of the leftover bytes, at most a few (D-025) |
| 0.3 | An inflater that accepts what zlib rejects is a parser differential: a proxy and a backend can disagree on what a body contains. Holding the inflater to zlib's exact acceptance rules (incomplete codes, missing end-of-block, over-long repeats) closes that gap. | Differential tests and the `inflate_raw` fuzz target compare the verdict and output with zlib (through zlib-rs) on corrupted and random streams. | none at run time (D-018) |
| 0.2 | A checksum that is merely "verified" can still be skipped by a caller who forgets to finish the stream; the framing decoders verify the trailer inside `finish`, and the one-shot functions always call it, so no caller-facing path returns data without its checksum checked. | 0.5 tests: every Tier 1 decoder fed a stream with a flipped trailer byte fails, and no public decoder returns `Done` before the trailer is read. | none: the checksum runs anyway |
| 0.1 | The ratio cap must count input the inflater has taken into its bit buffer, not input it has decoded, or an attacker can hold bytes pending and make the measured ratio look smaller than it is. | `total_in` counts bytes moved into the bit buffer; a test feeds one byte per call and checks the ratio error fires at the same output position as a one-shot call. | none: the counter exists anyway |

---

## 16. Decision log

| Id | Date | Decision | Reason |
|---|---|---|---|
| D-001 | 2026-11-05 | Crate `compress-lib`, a `-lib` (toolbox, no threads or timers), public repository `jamesgober/compress-lib` | James named it; the name was free on crates.io and GitHub |
| D-002 | 2026-11-05 | MSRV 1.85, edition 2024 | the portfolio default; nothing newer is needed (`core::error::Error` is 1.81) |
| D-003 | 2026-11-05 | `#![forbid(unsafe_code)]` | a decoder of hostile input is where memory safety matters most; speed comes from table design and copy strategy (OD-1) |
| D-004 | 2026-11-05 | Zero runtime dependencies in every feature mode, checked in CI; `miniz_oxide` only as a dev-dependency (differential oracle); competitor crates only in the unpublished `bench-compare/` crate | "ours, not rented"; competitors with C toolchains must not reach the published crate's dependency graph |
| D-005 | 2026-11-05 | No async runtime; the REPS clause that requires tokio is an approved exception | a codec has no tasks; the sans-IO engine works under any driver |
| D-006 | 2026-11-05 | Engine shape: `decompress(input, output) -> Progress { consumed, produced, status }` with `NeedsInput`, `NeedsOutput`, `Done`, plus `finish` for the last input and `*_vec` variants that append | push input, pull output, caller-owned buffers: the shape every consumer (ws-net, http-net, serve-net) can drive without copies |
| D-007 | 2026-11-05 | Default `Limits`: 64 MiB absolute output, ratio cap off, ratio floor 1 MiB; `Limits::unlimited()` is a named opt-out | fail closed by default; DEFLATE's maximum ratio (about 1032:1) makes a default ratio cap either useless or harmful to legitimate data (OD-2, OD-3) |
| D-008 | 2026-11-05 | Levels 0 to 9 with zlib's meaning | operators and consumers keep their intuition (OD-4) |
| D-009 | 2026-11-05 | Order: checksums 0.2, inflate 0.3, deflate and RFC 7692 0.4, zlib and gzip 0.5; brotli decode 1.1, encode 1.2 behind a non-default feature | ws-net's permessage-deflate is the first consumer and needs both directions of raw DEFLATE; HTTP needs gzip next (OD-6) |
| D-010 | 2026-11-05 | The 0.2 design lock is approved by this plan under James's standing instruction to decide technical matters from the requirements; only 1.0 waits for his explicit go | the requirements fix the API shape; nothing in it is a product, money, or visibility call |
| D-011 | 2026-11-05 | `no_std` with `alloc` for the codecs; `std` (default) adds the `io` adapters | embedded and wasm users; the codecs need no OS |
| D-012 | 2026-11-05 | Benchmark corpora are generated deterministically in the benchmark code plus the repository's own files; no corpus download | reproducible on every machine and runner, nothing fetched from outside |
| D-013 | 2026-11-05 | CI: Linux, macOS, Windows on x86_64 and aarch64, stable and 1.85; cross checks for i686, s390x (big-endian), wasm32, and a bare-metal `no_std` target; Miri on x86_64 and s390x; a job that fails on any runtime dependency | the bit reader packs bytes into words, so width and byte order are tested, not assumed |
| D-014 | 2026-11-06 | `ErrorKind::AllocationFailed`: decoders that grow a `Vec` use `try_reserve` and fail with it instead of aborting | a server under memory pressure should refuse one request, not die; the cost is one branch per reservation |
| D-015 | 2026-11-06 | `ErrorKind::InvalidState` added to the locked set (E-26: `next_stream` before `Done`) and `Error::offset` counts compressed input since the engine started or was reset | errors must say where; state misuse must be an error, never a panic |
| D-016 | 2026-11-06 | CRC-32 by slicing-by-16 tables built at compile time (16 KiB static); Adler-32 with four lanes and deferred modulo (zlib's NMAX 5,552 per lane) | portable safe-Rust speed on every target; hardware CRC (PCLMULQDQ, ARMv8 CRC) needs `unsafe` intrinsics or runtime detection and is left to OD-1 |
| D-017 | 2026-11-06 | `criterion` as a dev-dependency with default features off; competitor benchmarks stay in `bench-compare/` (0.6) | the crate's own baselines live with the code; competitors pull in C toolchains |
| D-018 | 2026-11-06 | The inflater follows zlib's acceptance rules exactly: over-subscribed codes rejected; incomplete codes rejected except a single code of length 1 for literal/length or distance; the precode must be complete; end-of-block must have a code; HLIT above 286 and HDIST above 30 rejected | the same verdict as zlib everywhere avoids parser differentials between this crate and the C and zlib-rs decoders the rest of the web runs (idea 0.3); miniz_oxide is laxer on incomplete codes |
| D-019 | 2026-11-06 | Output goes straight into the caller's buffer; history beyond it comes from a ring of `2^window_bits` bytes updated once per call; one-shot decoding allocates no ring; `decompress_vec` uses the vector as history for the whole call | no staging copy, `O(produced)` ring cost per call whatever the buffer sizes, and no 32 KiB window for one-shot callers |
| D-020 | 2026-11-06 | zlib-rs joins miniz_oxide as a test-only oracle (dev-dependency and fuzz crate, never runtime): zlib-rs for exact verdicts, miniz_oxide for streams at every level | zlib's rules are the reference (D-018); zlib-rs is pure Rust, so the oracle needs no C toolchain |
| D-021 | 2026-11-06 | `decompress_vec` grows by doubling, never past `max_output`; a reallocation can briefly hold the old buffer and the new one, so peak memory is at most 1.5 times the cap; the spec's "cap plus one chunk" is restated | chunk-sized exact growth would copy quadratically; doubling bounded by the cap keeps both copies and peak memory linear |
| D-022 | 2026-11-06 | Once the cap is spent and the stream still has output pending, the inflater fails with `OutputLimit` even when the caller's buffer ended exactly at the cap | otherwise the vector API would ask for room it can never get and loop |
| D-023 | 2026-11-06 | Fast loop: one branch-free eight-byte refill per symbol (libdeflate's method), a second literal per refill, eight-byte match copies for distances of eight or more; the slow path keeps the bit buffer clean above its count | the measured 0.3 baseline is ahead of miniz_oxide on most rows; tuning to the section 9 targets is 0.6 |
| D-024 | 2026-11-06 | `Inflater::next_stream`, `set_dictionary`, and `memory_in_use` (DF-17 to DF-19) ship in 0.3 instead of 0.4 | they are inflater state only, done with the inflater, and ws-net can wire takeover against 0.3 |
| D-025 | 2026-11-06 | `message::Decompressor` accepts zero bytes after a final block and refuses anything else (`TrailingData`) | RFC 7692 section 7.2.3.4's example payload ends with one zero byte after the final block, and zlib-based receivers ignore it; nonzero bytes there have no meaning and are refused (idea 0.4) |
| D-026 | 2026-11-06 | Level settings are zlib's table exactly (good, lazy, nice, chain); the earlier draft table is replaced | sizes comparable level for level with zlib, which the tests check (within 1.5% on text, in practice equal or smaller) |
| D-027 | 2026-11-06 | Deflater window: a buffer of `max(2, 8192 / 2^bits)` windows; the full window is used for distances when the buffer holds three or more windows (bits 8 to 11), and `2^bits - 262` otherwise (zlib's MAX_DIST); positions start at 1 so position 0 can mean "none" in the 16-bit chains | 16-bit chains at every window size; RFC 7692's small windows get their whole window; the large-window loss is zlib's |
| D-028 | 2026-11-06 | Hash bits `clamp(window bits + 1, 9, 15)`, symbol buffer `clamp(2^(bits + 1), 1024, 16384)` | small windows mean memory-constrained connections; their tables shrink with the window |
| D-029 | 2026-11-06 | Every Huffman block carries at least two literal/length and two distance codes (dummy frequencies, as zlib's trees.c) | no decoder ever meets the single-code or empty-code special cases from this encoder |
| D-030 | 2026-11-06 | `set_level` across level 0 ends the block first; from lazy to greedy it writes the held-back literal; any change clears the pending match | the strategies keep different state; a change must never drop or repeat a byte (found by `test_roundtrip_set_level_mid_stream`) |
| D-031 | 2026-11-06 | Deflate speed tuning moves to 0.6 with the rest of the optimisation milestone; 0.4 ships with sizes at or below zlib's and speed recorded as a gap (level 1 about a third of miniz_oxide's on the shared machine) | correctness and ratio first; 0.6 is where the section 9 targets are met or the gap is published |
| D-032 | 2026-11-06 | Level 0 writes a stored block whenever the window cannot take more input; `Deflater::compress` returns `InvalidState` if a round ever takes no input and makes no output instead of repeating | the property test found a stall at level 0 with a 4 KiB window after flushes; the guard turns any such state into an error, never a hang (cost: one tuple compare per round) |
| D-033 | 2026-11-06 | The zlib decoder checks the header's window (CINFO) against its configured maximum (`InvalidWindowBits` above it) and decodes with the configured window, not the declared one | refusing what the caller cannot afford is the defence; distances within the configured window are what zlib's one-shot decoding accepts too, so valid-but-sloppy encoders interoperate |
| D-034 | 2026-11-06 | gzip decoding: multi-member by default with `max_members`; after a member `decompress` returns `NeedsInput` (another may follow) and `finish` ends the stream; `with_single_member` stops at the first; the output cap is shared by all members (idea 0.5); bytes after the last member are `TrailingData` unless `with_trailing_data(true)` | RFC 1952 section 2.2 allows members back to back and gzip tools write them; a caller who knows its data is one member can say so |
| D-035 | 2026-11-06 | gzip header fields (FEXTRA, FNAME, FCOMMENT) count against `max_header` together and are stored only with `with_keep_header(true)`; FHCRC is always verified | header memory is the attacker's to choose; by default none is spent |
| D-036 | 2026-11-06 | The gzip encoder writes time stamp 0, OS 255 ("unknown"), and XFL from the level by default | the same input gives the same bytes on every platform (NFR-7); callers who want a name or time set `GzipHeader` |
| D-037 | 2026-11-06 | The zlib decoder keeps a copy of a preset dictionary and applies it only when the header names one and its Adler-32 matches | a dictionary given in advance must not widen what a stream without FDICT may refer to |
| D-038 | 2026-11-06 | `io` adapters hold one 32 KiB buffer each; `Writer` tries to finish on drop and ignores errors (documented: call `finish`); `Reader` returns 0 at the end of the stream and `UnexpectedEof` for a truncated source | flate2's behaviour, which users expect; errors only `finish` can report are reported there |
| D-039 | 2026-11-06 | `Format::detect` recognises gzip by magic and zlib by header check and never guesses raw DEFLATE; `compress_lib::decompress` refuses anything else with `InvalidHeader` | a raw guess would turn arbitrary bytes into "valid" output |
| D-040 | 2026-11-06 | Level 1 is a single-probe greedy matcher over a 4,096-entry hash table (it stays in the first-level cache), and after 64 literals in a row it searches only every few positions, more sparsely the longer the run; zlib's level 1 (four probes, 32,768 entries) was 3 to 4 times slower than miniz_oxide | measured: text 324,322 bytes (was 290,167; miniz_oxide 347,486, zlib-rs 555,999), random input about 1.4 times miniz_oxide's speed instead of a quarter of it |
| D-041 | 2026-11-06 | The greedy and lazy loops hold their state in locals and compare candidates four bytes at a time (and the two bytes at the end of a longer match first); the window carries 8 padding bytes so four-byte loads stay in bounds | no `unsafe`; 15 to 30% faster at every level with the same match decisions |
| D-042 | 2026-11-06 | The block writer keeps its bit buffer in locals and writes a symbol's code and extra bits in one operation | block writing was a third of level 1's time |
| D-043 | 2026-11-06 | `Deflater::reset` re-hashes the positions a short, unslid stream used and clears only those heads | per-message compression without takeover cleared 64 KiB per message |
| D-044 | 2026-11-06 | Vector decoding starts with four times the input (256 B to 64 KiB) and then doubles | small messages decoded with a 64 KiB zero-filled reservation each (7.3 to 2.4 microseconds per 70-byte message) |
| D-045 | 2026-11-06 | Section 9 targets: met for decoding against miniz_oxide on most inputs and for sizes everywhere; not met for compression speed against zlib-rs at levels 6 and 9, nor 1.2 times miniz_oxide on every row. Recorded as the gap; closing it needs SIMD-style match finding that `forbid(unsafe_code)` excludes (OD-1) or a different match finder (binary trees, as libdeflate), which is a 1.x candidate | standard 1.2: publish the gap, never assert a number that was not measured |
| D-046 | 2026-11-06 | Encoder positions start at 1 (position 0 is "none" in the chains), as D-027 states; until 0.6 they started at 0 | the first byte of a stream (or of a message with takeover) can now be matched |
