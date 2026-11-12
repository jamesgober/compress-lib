# Architecture

How compress-lib is built: the modules, how data moves through them, and the design choices behind each. The decision log in `dev/PLAN.md` records why each choice was made.

## Layers

```text
                         one call: deflate::compress, gzip::decompress, decompress(...)
                                                    |
      io::DecodeReader / EncodeWriter / EncodeReader / DecodeWriter   (feature std)
                                                    |
       Decoder / Encoder (any Format)       message::Compressor / Decompressor (RFC 7692)
                         |                                   |
           zlib::{Decoder, Encoder}     gzip::{Decoder, Encoder}
                         \                 /
                  deflate::Inflater      deflate::Deflater
                  (src/inflate/)         (src/deflate/encoder.rs, huffman.rs)
                                \        /
                       checksum::{Crc32, Adler32}   Limits   Error
```

Every layer above the engines is a thin state machine around them. Nothing performs I/O except the `io` adapters, and they only call the `Read` or `Write` they wrap. There are no threads, no globals, and no `unsafe` code.

## The streaming contract

Every engine has the same shape: `decompress(input, output)` or `compress(input, output, flush)` returns `Progress { consumed, produced, status }`.

- `NeedsInput`: all input was consumed; call again with more.
- `NeedsOutput`: the output buffer is full; call again with more room and the input not yet consumed.
- `Done`: the stream (or the requested flush) is complete. A decoder's `consumed` stops at the stream's last byte, so trailing data (a zlib trailer, the next gzip member, another protocol's bytes) stays with the caller.

`finish` marks the last input: a decoder that has not reached the end then fails with `UnexpectedEnd`. The `_vec` variants append to a vector, growing it within the limits.

## Inflater (`src/inflate/`)

- **Bit reader.** A 64-bit buffer. In the fast loop it is refilled eight bytes at a time without a branch (libdeflate's method): the bytes are read, the buffer gets `(63 - count) / 8` of them, and the count becomes `count | 56`. Bits above the count are kept clear whenever the slow path runs.
- **Tables.** Literal/length codes use an 11-bit primary table with subtables (2,342 entries at most), distances an 8-bit table (402), code lengths a 7-bit table. An entry packs the bits to consume (code and extra bits together), the code length, a kind flag, and the value, so one shift consumes both. The fixed-code tables are built at compile time. The table builder follows zlib's acceptance rules exactly (D-018).
- **Output and history.** Output goes straight into the caller's buffer. Back-references reach into the output of the same call, and beyond it into a ring of `2^window_bits` bytes updated once per call with the newest output (D-019). The vector paths use the vector itself as history, so one-shot and per-message decoding keep no window at all.
- **Fast and slow paths.** With at least 8 input bytes and 274 output bytes of room, the fast loop decodes symbols with no state saved between them, two literals per refill when they fit, and matches in eight-byte copies: from the distance itself when it is eight or more, a fill for distance 1, and for distances 2 to 7 overlapping copies that build the first multiple of the period of at least eight bytes and then copy from that far back (D-058). Otherwise the slow path decodes one symbol at a time and can stop anywhere: inside a header, a code, a match, or a stored block.
- **Vector output.** The `_vec` paths offer room in steps sized from the call's own input and doubled only within the call, use capacity already reserved before growing, and grow the vector geometrically up to the cap (D-054, D-058), so a call costs its own input and output and a stream that fits its reservation never reallocates.
- **Limits.** Before each step the output slice is cut to what the absolute cap and the ratio cap allow; a stream that needs more output fails with `OutputLimit` or `RatioLimit`, and nothing is written past the cap (D-022).

## Deflater (`src/deflate/encoder.rs`)

- **Window.** A buffer of at least 8 KiB, a whole number of windows, padded by 8 bytes for four-byte loads. It slides by whole windows, keeping the full window of history when the buffer holds three or more windows and `2^bits - 262` bytes otherwise (zlib's MAX_DIST), so positions fit the 16-bit hash chains (D-027). Positions start at 1; 0 means "none".
- **Match finding.** A multiplicative hash indexes chains of earlier positions: of three bytes at levels 1 to 3, of four at levels 4 to 9, where a three-byte prefix is too common to tell candidates apart and a three-byte match is rarely worth emitting (D-065). Levels 1 to 3 match greedily, 4 to 9 lazily (zlib's deflate_slow), with zlib's settings per level (D-026). The lazy search also spends from a budget of 64 chain steps per input byte, banked up to one window's worth: ordinary data never runs it down, and input built to fill every chain with short matches is held to about the search ordinary data gets (D-065). Level 1 probes once in a 4,096-entry table, keeps no chains, writes a three-byte match more than 8 KiB back as literals, and searches more sparsely through long runs of misses (D-040, D-055). A chain walk compares the four bytes that end a longer match before anything else, since a candidate that differs there cannot win, and is bounded by the window once per walk (D-058). The loops keep their state in locals and compare four and eight bytes at a time (D-041).
- **Level 0.** Stored blocks of a fixed size (16 KiB, or half the buffer for small windows); a shorter block only at a flush or when the window is full and cannot slide yet (D-057).
- **Blocks.** Symbols collect in a buffer (16,384 by default, fewer for small windows). A block ends when the buffer fills or at a flush, and is written as stored, fixed, or dynamic Huffman, whichever is smallest by exact bit count. Dynamic codes are built by Huffman's algorithm with two queues and length-limited by a Kraft-sum repair (15 bits for literals and distances, 7 for code lengths); every block carries at least two codes of each kind (D-029). The block writer builds per-block tables (a literal's code and length in one entry, a match length's code with its extra bits in another) and writes each symbol with one eight-byte store into room sized for the block (D-042, D-056).
- **Flushes.** `Sync` ends the block and writes an empty stored block (`00 00 FF FF`); `Full` does the same and clears the hash heads; `Finish` writes the final block. Output waits in a pending buffer until the caller drains it, and the engine stops taking input while output is pending. The buffer is sized at build for at least the largest block (4 bytes per symbol, which bounds the fixed-code size every block written is at most); each block reserves its exact bit count, and the engine stops for a drain whenever the room left could not take another largest block, so the buffer never grows (D-064).
- **Determinism.** The compressed bytes depend only on the input, the settings, and the flush sequence, never on how the input was split or how large the output buffers were: every piece of matcher state is kept across calls (D-057), and the `roundtrip` fuzz target checks it by compressing each input twice with different splits.

## Framing (`src/zlib.rs`, `src/gzip.rs`)

Byte-at-a-time state machines for headers and trailers around the engines, so a stream may be split anywhere. zlib checks CMF/FLG, the window size against the configured maximum (D-033), and a dictionary by its Adler-32 id (D-037). gzip parses every header flag with the optional fields bounded and stored only when asked (D-035), verifies the header CRC, the CRC-32, and the length, and continues across members with the output cap shared by all of them (D-034).

## Per-message compression (`src/message.rs`)

`Compressor` sync-flushes each message and removes the trailing `00 00 FF FF`; without takeover it resets after every message (clearing only the hash entries the message used, D-043). `Decompressor` decodes each message into the caller's vector, with the vector as the message's history and the budget as the inflater's output cap; it appends the tail, accepts a final block with zero padding after it (D-025), and keeps the window into the next message only with takeover.

## Errors and limits

`Error` is a kind and an input offset, `Copy`, and never carries data from the stream. Kinds have stable codes and messages that say what to do. `Limits` is a `Copy` value read by every decoder; every check runs before the work it guards.
