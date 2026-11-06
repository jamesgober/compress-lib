# Security

compress-lib decodes data an attacker controls: HTTP bodies, WebSocket messages, uploaded files. This file is its threat model: what each defence stops, what it costs, and the test that proves it. Every threat has an attack test in `tests/attacks.rs` named after it (`test_attack_t1_...`).

## Reporting

Report a vulnerability privately through GitHub's security advisories for `jamesgober/compress-lib`, or by email to me@jamesgober.com. Please include a reproducing input.

## What the crate guarantees

- **No `unsafe` code** (`#![forbid(unsafe_code)]`) and no runtime dependencies: memory safety rests on the Rust compiler and the standard library alone.
- **No panics on any input.** Every decoder returns an error for every malformed stream; the fuzz targets and the property tests check it.
- **The same verdict as zlib.** The inflater accepts exactly the streams zlib accepts and produces the same output (differential tests and the `inflate_raw` fuzz target against zlib-rs), so a proxy using compress-lib and a backend using zlib cannot disagree on what a body contains.
- **Fail closed by default.** Output is capped at 64 MiB, checksums are always verified, bytes after the end of a stream are refused, and a failed decoder stays failed until `reset`.

## Threats and defences

| Id | Threat | Defence | Cost | Test |
|---|---|---|---|---|
| T-1 | Decompression bomb: a few kilobytes expand to gigabytes | `Limits::max_output` (64 MiB by default) is checked before every output chunk is written, never after; a vector grows by doubling but never past the cap, so peak memory is at most 1.5 times the cap | one compare per output chunk (a literal run, a match, or a stored segment) | `test_attack_t1_bomb_stops_at_cap_in_every_format`, `test_alloc_bomb_peak_bounded_by_cap` |
| T-2 | A bomb that stays under the absolute cap, repeated across many requests | optional `Limits::max_ratio`, checked per chunk once the output passes `ratio_floor` (1 MiB by default); input still held in the bit buffer counts as consumed, so feeding one byte at a time cannot game it | one multiply and compare per chunk | `test_attack_t2_ratio_bomb_caught_by_ratio_cap`, `test_inflate_ratio_cap_counts_buffered_input_byte_at_a_time` |
| T-3 | Malformed Huffman tables (over-subscribed, incomplete, bad repeats) that make decoders read out of bounds or loop | the table builder checks the Kraft sum, completeness (zlib's rules), and every repeat against the table's end; unused bit patterns decode to an invalid entry | none while decoding symbols; a few hundred operations per table | `test_attack_t3_malformed_tables_refused`, unit tests in `src/inflate/tests.rs` |
| T-4 | A back-reference before the start of the data or beyond the window (stale memory in C decoders) | every distance is checked against the output and history available and against the window size (window bits 8 to 15, exactly) | one compare per match | `test_attack_t4_distance_beyond_data_or_window` |
| T-5 | Quadratic work from tiny output buffers or byte-by-byte input | the history ring is updated once per call with the newest output, `O(produced)`, never `O(window)` | none | `test_attack_t5_tiny_buffers_linear` |
| T-6 | Endless empty blocks or empty gzip members | every loop consumes input or produces output; gzip members are capped by `Limits::max_members` (1,024) | one counter per member | `test_attack_t6_empty_blocks_and_members_bounded` |
| T-7 | Huge gzip header fields (name, comment, extra) to exhaust memory | the fields count against `Limits::max_header` (64 KiB) together and are stored only with `with_keep_header(true)` | one counter per header byte | `test_attack_t7_gzip_header_bounded` |
| T-8 | Corrupted or forged data accepted as good | Adler-32, CRC-32, ISIZE, and the gzip header CRC are always verified; there is no option to skip them | the checksum, measured as part of every gzip and zlib benchmark in `docs/PERFORMANCE.md` | `test_attack_t8_checksums_always_verified` |
| T-9 | Compression side channels (CRIME, BREACH): secrets compressed with attacker-chosen data leak through the compressed size | `message` defaults to no context takeover, so every message is compressed alone; `Flush::Full` cuts history mid-stream; the guidance below | some ratio between related messages | `test_attack_t9_no_takeover_no_cross_message_oracle` |
| T-10 | Many connections each holding large compression state | memory per engine is fixed when it is built and reported by `memory_in_use()`; a decompressor without takeover keeps no window (about 11 KiB); `MemoryConfig` shrinks a compressor | none | `test_attack_t10_memory_fixed_per_engine`, `docs/PERFORMANCE.md` (memory table) |
| T-11 | A panic taking down a server thread | no `unwrap`, `expect`, or `panic!` in library code (denied by lints); every index is bounded by a check the tests and fuzz targets exercise | none | `test_attack_t11_no_panic_on_arbitrary_input`, the fuzz targets |
| T-12 | A truncated stream treated as complete (silent data loss) | `finish` and every one-shot function report `UnexpectedEnd` for an unfinished stream | none | `test_attack_t12_truncation_detected` |
| T-13 | Data smuggled after the end of a stream | one-shot functions and the gzip decoder refuse trailing bytes (`TrailingData`); streaming decoders stop `consumed` at the stream's last byte; after a final block in a WebSocket message only zero padding is allowed | none | `test_attack_t13_trailing_data_refused` |
| T-14 | Encoder input built to make the match finder work hardest | every level has a fixed chain limit (4,096 probes per position at level 9) | none | `test_attack_t14_adversarial_encoder_input_bounded` |
| T-15 | Plaintext left behind in compression state (the window and buffers hold earlier messages) after a connection that carried secrets ends | `wipe()` on `Inflater`, `Deflater`, `message::Compressor`, and `message::Decompressor` zeroes the history, tables, and buffers, spare capacity included, through `core::hint::black_box`; without `unsafe` volatile writes this is a strong hint to the compiler, not a guarantee | one pass over the engine's memory (about 400 KiB for a level-6 deflater, 44 KiB for an inflater) | `test_attack_wipe_leaves_no_plaintext`, `test_deflater_wipe_zeroes_every_buffer`, `test_inflate_wipe_zeroes_window_and_tables` |

## Guidance

- **Set limits for your data.** The 64 MiB default stops bombs from taking a server down, but a JSON API that never accepts more than 1 MiB should say so: `Limits::new().with_max_output(1 << 20)`. When you know the data, a ratio cap catches bombs below the cap: JSON rarely compresses beyond 20 to 1, so `with_max_ratio(50)` is generous.
- **Do not compress secrets together with attacker-controlled data.** This is a property of compression, not of this crate. For WebSocket, keep context takeover off (the default) when messages mix secrets with data a peer controls, and let the WebSocket layer send such messages uncompressed. For HTTP responses that reflect user input next to a CSRF token or session data, do not compress those responses (BREACH).
- **Treat `Limits::unlimited()` as a review flag.** It exists for your own files; it should not appear on a path that reads network input.
- **Errors are safe to log.** An `Error` holds a kind and an input offset, never data from the stream.

## What is out of scope

- Authentication: a checksum detects accidental corruption, not tampering. Use a MAC or a signature for integrity against an attacker.
- Timing: decoding time depends on the data. Do not decompress secrets in code whose timing an attacker can measure if that matters to you.
