<h1 align="center">
    <img width="90px" height="auto" src="https://raw.githubusercontent.com/jamesgober/jamesgober/main/media/icons/hexagon-3.svg" alt="Triple Hexagon">
    <br><b>compress-lib API</b>
</h1>

The complete public API planned for 1.0, each item with the version it arrives in. Items marked with a later version do not exist in the current release; the crate exposes only what is implemented. Item ids match `dev/PLAN.md` section 6.

Add the crate with Cargo's rename so code reads `compress::`:

```toml
[dependencies]
compress = { package = "compress-lib", version = "0.6" }
```

## Contents

- [Tier 1: one call](#tier-1-one-call)
- [Root](#root)
  - [`VERSION`](#version)
  - [`Error` and `ErrorKind`](#error-and-errorkind)
  - [`Limits`](#limits)
  - [`Level`](#level)
- [`checksum`](#checksum)
  - [`crc32`, `Crc32`, `crc32_combine`](#crc32-crc32-crc32_combine)
  - [`adler32`, `Adler32`, `adler32_combine`](#adler32-adler32-adler32_combine)
- [`deflate`: raw DEFLATE](#deflate-raw-deflate)
  - [One-shot functions](#one-shot-functions)
  - [`Inflater`](#inflater)
  - [`Deflater`](#deflater)
  - [`Progress`, `Status`, `Flush`](#progress-status-flush)
- [`message`: WebSocket permessage-deflate](#message-websocket-permessage-deflate)
- [`zlib` and `gzip`](#zlib-and-gzip)
- [`Format`, `Decoder`, `Encoder`](#format-decoder-encoder)
- [`io` adapters](#io-adapters)
- [Brotli (after 1.0)](#brotli-after-10)
- [Features](#features)

---

## Tier 1: one call

```rust
let packed = compress::gzip::compress(body);                 // 0.5
let body = compress::gzip::decompress(&packed)?;             // 0.5, capped at 64 MiB
let raw = compress::deflate::compress(data);                 // 0.4
let data = compress::deflate::decompress(&raw)?;             // 0.3
let z = compress::zlib::compress(data);                      // 0.5
let crc = compress::checksum::crc32(data);                   // 0.2
```

Decoders always finish the stream: truncated input is `UnexpectedEnd`, bytes after the end are `TrailingData`, and output past `Limits::max_output` (64 MiB by default) is `OutputLimit`.

---

## Root

### `VERSION`

`pub const VERSION: &str` (ROOT-01, 0.1): the crate version.

### `Error` and `ErrorKind`

```rust
pub struct Error { /* kind, offset */ }
#[non_exhaustive] pub enum ErrorKind { /* below */ }
```

| Method | Returns | Since |
|---|---|---|
| `Error::kind(&self)` | the `ErrorKind` | 0.2 |
| `Error::offset(&self)` | the compressed-input byte offset where it was found (0 for configuration errors) | 0.2 |
| `Error::code(&self)`, `ErrorKind::code(self)` | a stable string such as `"compress.output_limit"`, never changed once released | 0.2 |
| `ErrorKind::message(self)` | one sentence: what happened and what to do | 0.2 |
| `Display`, `core::error::Error`, `From<Error> for std::io::Error` (feature `std`; `UnexpectedEnd` maps to `UnexpectedEof`, configuration errors to `InvalidInput`, the rest to `InvalidData`) | | 0.2 |

| Kind | When | Used from |
|---|---|---|
| `InvalidBlockType` | block type 3 | 0.3 |
| `InvalidStoredLength` | LEN and NLEN disagree | 0.3 |
| `InvalidCodeLengths` | over-subscribed or forbidden incomplete code, bad repeat | 0.3 |
| `InvalidSymbol` | litlen 286/287, distance 30/31, unused code | 0.3 |
| `DistanceTooFar` | back-reference before the data (and dictionary) or beyond the window | 0.3 |
| `UnexpectedEnd` | input ended inside the stream | 0.3 |
| `OutputLimit` | `max_output` reached | 0.3 |
| `RatioLimit` | `max_ratio` exceeded | 0.3 |
| `TrailingData` | bytes after the end where none are allowed | 0.3 |
| `InvalidWindowBits` | window bits outside 8 to 15 | 0.3 |
| `InvalidLevel` | level above 9 | 0.2 |
| `InvalidHeader` | zlib or gzip header malformed | 0.5 |
| `ChecksumMismatch` | Adler-32, CRC-32, or header CRC differs | 0.5 |
| `LengthMismatch` | gzip ISIZE differs | 0.5 |
| `HeaderTooLarge` | gzip optional fields past `max_header` | 0.5 |
| `TooManyMembers` | gzip members past `max_members` | 0.5 |
| `DictionaryRequired` | zlib FDICT without `set_dictionary` | 0.5 |
| `WrongDictionary` | dictionary id differs | 0.5 |
| `InvalidState` | a call the engine's state does not allow | 0.3 |
| `AllocationFailed` | a decoder could not grow its output `Vec` (`try_reserve` failed) | 0.3 |

```rust
match compress::deflate::decompress(&body) {
    Ok(data) => use_it(data),
    Err(e) if e.kind() == compress::ErrorKind::OutputLimit => reject_413(),
    Err(e) => reject_400(e.code()),
}
```

### `Limits`

`#[non_exhaustive] pub struct Limits` (LIM-01 to LIM-05). All methods are `const fn`.

| Item | Default | Since |
|---|---|---|
| `Limits::new()`, `Default` | see below | 0.2 |
| `Limits::unlimited()` | no output or ratio cap, members unbounded | 0.2 |
| `with_max_output(u64)`, `max_output()` | 64 MiB (`DEFAULT_MAX_OUTPUT`) | 0.2 |
| `with_max_ratio(u32)`, `max_ratio()` | 0 (off) | 0.2 |
| `with_ratio_floor(u64)`, `ratio_floor()` | 1 MiB (`DEFAULT_RATIO_FLOOR`) | 0.2 |
| `with_max_header(usize)`, `max_header()` | 64 KiB (`DEFAULT_MAX_HEADER`) | 0.2 (enforced 0.5) |
| `with_max_members(u32)`, `max_members()` | 1024 (`DEFAULT_MAX_MEMBERS`) | 0.2 (enforced 0.5) |

```rust
use compress::Limits;

// An API gateway that only expects JSON bodies up to 8 MiB.
let limits = Limits::new().with_max_output(8 << 20).with_max_ratio(50);
let body = compress::gzip::decompress_with(&wire, limits)?;
```

Costs: one compare per output chunk (absolute), one multiply and compare per chunk (ratio).

### `Level`

`pub struct Level` (LVL-01, 0.2): `Level::NONE` (0), `FAST` (1), `DEFAULT` (6), `BEST` (9), `Level::new(u8) -> Result<Level, Error>` (`InvalidLevel` above 9), `get()`, `TryFrom<u8>`, `Default` (6), ordered.

---

## `checksum`

### `crc32`, `Crc32`, `crc32_combine`

| Item | Behaviour | Since |
|---|---|---|
| `crc32(&[u8]) -> u32` | CRC-32/ISO-HDLC (gzip, zip), slicing-by-16 | 0.2 |
| `Crc32::new()`, `from_value(u32, u64)`, `update(&mut self, &[u8])`, `finish(&self) -> u32`, `len()`, `is_empty()`, `combine(&mut self, &Crc32)`, `Default` | streaming | 0.2 |
| `crc32_combine(crc1, crc2, len2: u64) -> u32` | CRC of A then B in O(log len2) | 0.2 |

```rust
use compress::checksum::{Crc32, crc32};

assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
let mut crc = Crc32::new();
for piece in pieces { crc.update(piece); }
let value = crc.finish();
```

### `adler32`, `Adler32`, `adler32_combine`

Same shape as CRC-32 (`adler32`, `Adler32::{new, from_value, update, finish, len, is_empty, combine}`, `adler32_combine`), 0.2. Four lanes with deferred modulo.

---

## `deflate`: raw DEFLATE

### One-shot functions

| Item | Since |
|---|---|
| `deflate::decompress(&[u8]) -> Result<Vec<u8>, Error>` (DF-01) | 0.3 |
| `deflate::decompress_with(&[u8], Limits) -> Result<Vec<u8>, Error>` (DF-02) | 0.3 |
| `Inflater::finish_vec` (as `decompress_vec`, last input) | 0.3 |
| `deflate::compress(&[u8]) -> Vec<u8>` (DF-03, level 6) | 0.4 |
| `deflate::compress_with(&[u8], Level) -> Vec<u8>` (DF-04) | 0.4 |

### `Inflater`

The streaming decoder (DF-10 to DF-19). Push input, pull output, caller-owned buffers; memory fixed at construction.

| Item | Behaviour | Since |
|---|---|---|
| `Inflater::new()`, `Default` | window bits 15, default limits | 0.3 |
| `with_window_bits(self, u8) -> Result<Self, Error>` | 8 to 15; a distance above `2^bits` is `DistanceTooFar` | 0.3 |
| `with_limits(self, Limits) -> Self` | | 0.3 |
| `decompress(&mut self, input: &[u8], output: &mut [u8]) -> Result<Progress, Error>` | more input may follow | 0.3 |
| `finish(&mut self, input: &[u8], output: &mut [u8]) -> Result<Progress, Error>` | this is the last input; an unfinished stream is `UnexpectedEnd` once output is not the reason | 0.3 |
| `decompress_vec(&mut self, input: &[u8], out: &mut Vec<u8>) -> Result<Progress, Error>` | appends, reserving at most 64 KiB at a time and never past the cap | 0.3 |
| `is_done()`, `total_in()`, `total_out()`, `window_bits()`, `limits()` | | 0.3 |
| `reset(&mut self)` | forget history and state, keep allocations | 0.3 |
| `next_stream(&mut self) -> Result<(), Error>` | after `Done`, start a new stream keeping the window (`InvalidState` before `Done`) | 0.3 |
| `set_dictionary(&mut self, &[u8]) -> Result<(), Error>` | preset history; only before any input | 0.3 |
| `memory_in_use(&self) -> usize` | heap bytes held | 0.3 |
| `wipe(&mut self)` | zero the window and decode tables (through `black_box`; no `unsafe`), then reset | 0.7 |

```rust
use compress::deflate::{Inflater, Status};

let mut inflater = Inflater::new();
let mut out = [0u8; 16 * 1024];
loop {
    let input = next_chunk();                       // from a socket, a file, a frame
    let mut rest = &input[..];
    loop {
        let p = inflater.decompress(rest, &mut out)?;
        rest = &rest[p.consumed..];
        sink(&out[..p.produced]);
        match p.status {
            Status::NeedsOutput => continue,
            Status::NeedsInput => break,
            Status::Done => return Ok(()),
            _ => break,
        }
    }
}
```

### `Deflater`

The streaming encoder (DF-30 to DF-37).

| Item | Behaviour | Since |
|---|---|---|
| `Deflater::new(Level)`, `Default` (level 6, window bits 15) | | 0.4 |
| `with_window_bits(self, u8) -> Result<Self, Error>` | 8 to 15; never emits a longer distance | 0.4 |
| `compress(&mut self, input, output, Flush) -> Result<Progress, Error>` | `Done` when the flush point is fully written | 0.4 |
| `compress_vec(&mut self, input, &mut Vec<u8>, Flush) -> Result<Progress, Error>` | appends until the flush point is written | 0.4 |
| `set_level(&mut self, Level)` | applies to data not yet compressed | 0.4 |
| `level()`, `window_bits()` | | 0.4 |
| `reset`, `set_dictionary`, `total_in`, `total_out`, `memory_in_use` | | 0.4 |
| `wipe(&mut self)` | zero the window, hash chains, symbols, pending output, and scratch, spare capacity included, then reset | 0.7 |
| `with_memory(self, MemoryConfig) -> Self`; `MemoryConfig::new()`, `with_hash_bits(u8)` (8 to 16), `with_block_symbols(usize)` (256 to 65,536), `hash_bits()`, `block_symbols()` (0 means chosen from the window bits) | hash table and symbol buffer sizes | 0.6 |
| `MessageConfig::with_memory(MemoryConfig)`, `memory()` | the same for the per-message compressor | 0.6 |

### `Progress`, `Status`, `Flush`

```rust
#[non_exhaustive] pub struct Progress { pub consumed: usize, pub produced: usize, pub status: Status }
#[non_exhaustive] pub enum Status { NeedsInput, NeedsOutput, Done }
#[non_exhaustive] pub enum Flush { None, Sync, Full, Finish }
```

`Sync` byte-aligns and writes an empty stored block (`00 00 FF FF`); `Full` does the same and forgets history; `Finish` writes the final block.

---

## `message`: WebSocket permessage-deflate

RFC 7692 section 7.2 on top of raw DEFLATE (MSG-01 to MSG-07, 0.4).

| Item | Behaviour |
|---|---|
| `MessageConfig::new()` | window bits 15, no context takeover, level 1 |
| `with_window_bits(u8)`, `with_context_takeover(bool)`, `with_level(Level)` | per direction: the sender's `*_max_window_bits` and `*_no_context_takeover` |
| `Compressor::new(MessageConfig)`; `compress_message(&mut self, msg: &[u8], out: &mut Vec<u8>) -> Result<usize, Error>` | sync flush, trailing `00 00 FF FF` removed; returns bytes appended |
| `Decompressor::new(MessageConfig)`; `decompress_message(&mut self, payload: &[u8], out: &mut Vec<u8>, max: usize) -> Result<usize, Error>` | appends the tail, writes into `out` with `max` checked per chunk; accepts a final block (zero padding after it, as in RFC 7692 7.2.3.4, is allowed; other bytes are `TrailingData`) and starts the next message with the window kept (takeover) or cleared |
| `decompress_fragment(&mut self, fragment, last: bool, out, max)` | the same for a message arriving in frames; every fragment appends to the same vector, which is the message's history (`InvalidState` if it changed) |
| `reset()`, `memory_in_use()` | |
| `wipe()` on both (0.7) | zero the history and buffers when a connection that carried secrets ends; without takeover an idle connection can drop its engines and build new ones later |

```rust
use compress::message::{Compressor, Decompressor, MessageConfig};

let config = MessageConfig::new().with_window_bits(12);
let mut tx = Compressor::new(config)?;
let mut rx = Decompressor::new(config)?;
let mut wire = Vec::new();
tx.compress_message(b"{\"hello\":\"world\"}", &mut wire)?;
let mut text = Vec::new();
rx.decompress_message(&wire, &mut text, 1 << 20)?;
```

---

## `zlib` and `gzip`

| Item | Since |
|---|---|
| `zlib::{compress, compress_with, decompress, decompress_with}` (ZL-01) | 0.5 |
| `zlib::Decoder`, `zlib::Encoder` with `set_dictionary`, `dictionary_id()` (ZL-02) | 0.5 |
| `gzip::{compress, compress_with, decompress, decompress_with}` (GZ-01) | 0.5 |
| `gzip::Decoder` (multi-member by default; `with_single_member()`, `with_trailing_data(bool)`), `gzip::Encoder` (`with_header(GzipHeader)`) (GZ-02) | 0.5 |
| `GzipHeader::new()`, `with_name`, `with_comment`, `with_extra`, `with_mtime`, `with_os`, `with_text`; `name()`, `comment()`, `extra()`, `mtime()`, `os()`, `extra_flags()`, `text()`; read through `Decoder::with_keep_header(true)` and `Decoder::header()` (GZ-03) | 0.5 |
| `gzip::Decoder::members()`, `with_limits`; `zlib::Decoder::with_window_bits`, `with_limits`; `zlib::Encoder::with_window_bits`; `gzip::Encoder::with_window_bits` | 0.5 |

HTTP's `deflate` content coding is zlib framing (RFC 9110 section 8.4.1.2): use `zlib`.

---

## `Format`, `Decoder`, `Encoder`

| Item | Since |
|---|---|
| `Format::{Raw, Zlib, Gzip}`, `Format::detect(&[u8]) -> Option<Format>` (gzip by magic, zlib by header check; raw is never guessed) (FMT-01) | 0.5 |
| `Decoder::new(Format)`, `Encoder::new(Format, Level)`: one type for code that picks the format at run time (FMT-02) | 0.5 |
| `compress_lib::decompress(&[u8])` and `decompress_with(&[u8], Limits)`: gzip or zlib, detected (FMT-03) | 0.5 |
| `Decoder::{with_limits, format, decompress, finish, decompress_vec, finish_vec, is_done, total_in, total_out, reset}`, `Encoder::{format, compress, compress_vec, reset}` | 0.5 |

---

## `io` adapters

Feature `std` (default), 0.5 (IO-01 to IO-03): `io::Reader<R: Read>` (decompresses while reading; `with_limits`, `into_inner`), `io::Writer<W: Write>` (compresses while writing; `flush()` is a sync flush, `finish()` ends the stream and returns the inner writer; a drop without `finish` tries to end the stream and ignores errors), `io::Encoder<R: Read>` (reads plain data, returns it compressed) and `io::DecodeWriter<W: Write>` (written compressed, writes plain; `finish()` checks the stream ended). Each takes a `Format` and, for decoders, `Limits`, and holds one 32 KiB buffer besides its engine.

---

## Brotli (after 1.0)

Feature `brotli` (not default): `brotli::decompress`, `brotli::Decoder` in 1.1 (BR-01); `brotli::compress`, `brotli::Encoder` in 1.2 (BR-02).

---

## Features

| Feature | Default | Adds |
|---|---|---|
| `std` | yes | `io` adapters, `From<Error> for std::io::Error`. Without it the crate is `no_std` with `alloc`. |
| `brotli` | no | brotli, from 1.1 |
