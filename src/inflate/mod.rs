//! The raw DEFLATE (RFC 1951) decoder.
//!
//! Output goes straight into the caller's buffer. Back-references reach
//! into the output written in the same call, and, further back, into a ring
//! of `2^window_bits` bytes updated once per call with the newest output, so
//! a call costs `O(consumed + produced)` however small the buffers are.

pub(crate) mod tables;

use alloc::{boxed::Box, vec::Vec};

use tables::{
    Alphabet, DIST_ROOT, END_OF_BLOCK, FIXED_DIST, FIXED_LIT, INVALID, LIT_ROOT, LITERAL, PRE_ROOT,
    SUBTABLE, Tables,
};

use crate::{Error, ErrorKind, Limits, Progress, Status};

/// Output room the fast loop needs per symbol: the longest match plus an
/// eight-byte over-copy, rounded up.
const FAST_OUT: usize = 258 + 16;
/// Input the fast loop needs per symbol: one eight-byte refill.
const FAST_IN: usize = 8;
/// Order in which code-length code lengths are stored (RFC 1951 3.2.7).
const PRECODE_ORDER: [u8; 19] = [
    16, 17, 18, 0, 8, 7, 9, 6, 10, 5, 11, 4, 12, 3, 13, 2, 14, 1, 15,
];
/// Most output `decompress_vec` reserves in one step.
const VEC_CHUNK: usize = 64 << 10;
/// Spare capacity worth decoding into before the vector grows.
const VEC_MIN_SPARE: usize = 4 << 10;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum State {
    /// Waiting for a block header (3 bits).
    Header,
    /// Stored block: skip to a byte boundary, then LEN and NLEN.
    StoredLength,
    /// Stored block: `stored_left` bytes to copy.
    Stored,
    /// Dynamic block: HLIT, HDIST, HCLEN.
    Counts,
    /// Dynamic block: code-length code lengths, `index` of `hclen` read.
    PrecodeLengths,
    /// Dynamic block: literal/length and distance code lengths, `index` of
    /// `hlit + hdist` read.
    CodeLengths,
    /// Decoding symbols.
    Data,
    /// A length was decoded (`pending_len`); its distance comes next.
    Distance,
    /// A match is being copied: `pending_len` bytes left at `pending_dist`.
    Copy,
    /// The final block ended.
    Done,
}

/// Why the decoding loop stopped.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Stop {
    Done,
    NeedInput,
    NeedOutput,
}

/// A streaming raw DEFLATE decoder (RFC 1951).
///
/// Push compressed input and pull decompressed output through buffers you
/// own. Memory is fixed when the inflater is built: the decode tables (about
/// 11 KiB) and a history window of `2^window_bits` bytes. Every limit in
/// [`Limits`] is checked before output is written.
///
/// # Examples
///
/// ```
/// use compress_lib::deflate::{Inflater, Status};
///
/// // "hello" compressed with fixed Huffman codes.
/// let compressed = [0xCB, 0x48, 0xCD, 0xC9, 0xC9, 0x07, 0x00];
/// let mut inflater = Inflater::new();
/// let mut out = [0u8; 64];
/// let p = inflater.finish(&compressed, &mut out)?;
/// assert_eq!(p.status, Status::Done);
/// assert_eq!(&out[..p.produced], b"hello");
/// # Ok::<(), compress_lib::Error>(())
/// ```
pub struct Inflater {
    limits: Limits,
    window_bits: u8,
    /// Keep a history ring across calls (off only for one-shot decoding).
    keep_history: bool,
    state: State,
    final_block: bool,
    /// The current block uses the fixed codes.
    fixed: bool,
    bitbuf: u64,
    bitcnt: u32,
    total_in: u64,
    total_out: u64,
    /// Bytes taken and produced before this stream by the container that
    /// owns the inflater (earlier gzip members), so `max_ratio` and its floor
    /// apply to the whole container stream.
    ratio_base_in: u64,
    ratio_base_out: u64,
    failed: Option<Error>,
    hlit: u16,
    hdist: u16,
    hclen: u16,
    index: u16,
    stored_left: u32,
    pending_len: u32,
    pending_dist: u32,
    tables: Box<Tables>,
    /// History ring: the newest `whave` bytes end just before `wpos`.
    ring: Vec<u8>,
    wpos: usize,
    whave: usize,
}

impl core::fmt::Debug for Inflater {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Inflater")
            .field("window_bits", &self.window_bits)
            .field("limits", &self.limits)
            .field("state", &self.state)
            .field("total_in", &self.total_in)
            .field("total_out", &self.total_out)
            .field("failed", &self.failed)
            .finish_non_exhaustive()
    }
}

impl Default for Inflater {
    fn default() -> Self {
        Self::new()
    }
}

/// Copies `n` bytes from `buf[src..]` to `buf[dst..]` front to back, so an
/// overlapping source repeats its pattern (LZ77 semantics). `src < dst`.
#[inline(always)]
fn copy_forward(buf: &mut [u8], src: usize, dst: usize, n: usize) {
    let dist = dst - src;
    if dist >= n {
        buf.copy_within(src..src + n, dst);
    } else if dist == 1 {
        let b = buf[src];
        buf[dst..dst + n].fill(b);
    } else {
        // `buf[src..dst]` repeats with period `dist`; every copy doubles it.
        let mut done = 0;
        while done < n {
            let chunk = (dist + done).min(n - done);
            buf.copy_within(src..src + chunk, dst + done);
            done += chunk;
        }
    }
}

impl Inflater {
    /// An inflater with window bits 15 and the default [`Limits`].
    ///
    /// # Examples
    ///
    /// ```
    /// let inflater = compress_lib::deflate::Inflater::new();
    /// assert_eq!(inflater.window_bits(), 15);
    /// ```
    #[must_use]
    pub fn new() -> Self {
        let mut inflater = Self::without_history(Limits::new());
        inflater.keep_history = true;
        inflater.ring = alloc::vec![0; 1 << 15];
        inflater
    }

    /// An inflater that keeps no history between calls: for one-shot
    /// decoding, where all output lands in one buffer.
    pub(crate) fn without_history(limits: Limits) -> Self {
        Self {
            limits,
            window_bits: 15,
            keep_history: false,
            state: State::Header,
            final_block: false,
            fixed: false,
            bitbuf: 0,
            bitcnt: 0,
            total_in: 0,
            total_out: 0,
            ratio_base_in: 0,
            ratio_base_out: 0,
            failed: None,
            hlit: 0,
            hdist: 0,
            hclen: 0,
            index: 0,
            stored_left: 0,
            pending_len: 0,
            pending_dist: 0,
            tables: Box::new(Tables::new()),
            ring: Vec::new(),
            wpos: 0,
            whave: 0,
        }
    }

    /// Sets the window size to `2^bits` bytes, 8 to 15. A back-reference
    /// further than the window is `DistanceTooFar` (RFC 7692 negotiates
    /// these sizes). Memory: the window shrinks with it.
    ///
    /// # Errors
    ///
    /// [`ErrorKind::InvalidWindowBits`] outside 8 to 15.
    ///
    /// # Examples
    ///
    /// ```
    /// use compress_lib::deflate::Inflater;
    ///
    /// let inflater = Inflater::new().with_window_bits(9)?;
    /// assert_eq!(inflater.window_bits(), 9);
    /// assert!(Inflater::new().with_window_bits(16).is_err());
    /// # Ok::<(), compress_lib::Error>(())
    /// ```
    pub fn with_window_bits(mut self, bits: u8) -> Result<Self, Error> {
        if !(8..=15).contains(&bits) {
            return Err(Error::config(ErrorKind::InvalidWindowBits));
        }
        self.window_bits = bits;
        if self.keep_history {
            self.ring = alloc::vec![0; 1 << bits];
        }
        self.reset();
        Ok(self)
    }

    /// Sets the limits checked while decoding.
    ///
    /// # Examples
    ///
    /// ```
    /// use compress_lib::{Limits, deflate::Inflater};
    ///
    /// let inflater = Inflater::new().with_limits(Limits::new().with_max_output(1 << 20));
    /// assert_eq!(inflater.limits().max_output(), 1 << 20);
    /// ```
    #[must_use]
    pub fn with_limits(mut self, limits: Limits) -> Self {
        self.limits = limits;
        self
    }

    /// The window size in bits.
    ///
    /// # Examples
    ///
    /// ```
    /// assert_eq!(compress_lib::deflate::Inflater::new().window_bits(), 15);
    /// ```
    #[must_use]
    pub fn window_bits(&self) -> u8 {
        self.window_bits
    }

    /// The limits in force.
    ///
    /// # Examples
    ///
    /// ```
    /// use compress_lib::{Limits, deflate::Inflater};
    ///
    /// assert_eq!(Inflater::new().limits(), Limits::new());
    /// ```
    #[must_use]
    pub fn limits(&self) -> Limits {
        self.limits
    }

    /// `true` once the final block has ended.
    ///
    /// # Examples
    ///
    /// ```
    /// use compress_lib::deflate::Inflater;
    ///
    /// let mut inflater = Inflater::new();
    /// assert!(!inflater.is_done());
    /// inflater.finish(&[0x03, 0x00], &mut [])?; // an empty stream
    /// assert!(inflater.is_done());
    /// # Ok::<(), compress_lib::Error>(())
    /// ```
    #[must_use]
    pub fn is_done(&self) -> bool {
        self.state == State::Done
    }

    /// Compressed bytes consumed since the inflater was built or reset.
    ///
    /// # Examples
    ///
    /// ```
    /// use compress_lib::deflate::Inflater;
    ///
    /// let mut inflater = Inflater::new();
    /// inflater.finish(&[0x03, 0x00], &mut [])?;
    /// assert_eq!(inflater.total_in(), 2);
    /// # Ok::<(), compress_lib::Error>(())
    /// ```
    #[must_use]
    pub fn total_in(&self) -> u64 {
        self.total_in
    }

    /// Decompressed bytes produced since the inflater was built or reset.
    ///
    /// # Examples
    ///
    /// ```
    /// use compress_lib::deflate::Inflater;
    ///
    /// let mut inflater = Inflater::new();
    /// let mut out = [0u8; 8];
    /// inflater.finish(&[0xCB, 0x48, 0xCD, 0xC9, 0xC9, 0x07, 0x00], &mut out)?;
    /// assert_eq!(inflater.total_out(), 5);
    /// # Ok::<(), compress_lib::Error>(())
    /// ```
    #[must_use]
    pub fn total_out(&self) -> u64 {
        self.total_out
    }

    /// Heap bytes this inflater holds: the tables and the window.
    ///
    /// # Examples
    ///
    /// ```
    /// use compress_lib::deflate::Inflater;
    ///
    /// let small = Inflater::new().with_window_bits(9)?;
    /// assert!(small.memory_in_use() < Inflater::new().memory_in_use());
    /// # Ok::<(), compress_lib::Error>(())
    /// ```
    #[must_use]
    pub fn memory_in_use(&self) -> usize {
        core::mem::size_of::<Tables>() + self.ring.capacity()
    }

    /// Forgets the stream and its history, keeping the allocations, so the
    /// inflater can decode an unrelated stream.
    ///
    /// # Examples
    ///
    /// ```
    /// use compress_lib::deflate::Inflater;
    ///
    /// let mut inflater = Inflater::new();
    /// inflater.finish(&[0x03, 0x00], &mut [])?;
    /// inflater.reset();
    /// assert!(!inflater.is_done());
    /// assert_eq!(inflater.total_in(), 0);
    /// # Ok::<(), compress_lib::Error>(())
    /// ```
    pub fn reset(&mut self) {
        self.state = State::Header;
        self.final_block = false;
        self.fixed = false;
        self.bitbuf = 0;
        self.bitcnt = 0;
        self.total_in = 0;
        self.total_out = 0;
        self.ratio_base_in = 0;
        self.ratio_base_out = 0;
        self.failed = None;
        self.wpos = 0;
        self.whave = 0;
    }

    /// Zeroes the history window and the decode tables (which hold what the
    /// stream decoded and how it was coded), then resets the inflater for a
    /// new stream. For connections that carried secrets: call it before the
    /// inflater is dropped or reused. The allocations are kept.
    ///
    /// The zeroing goes through [`core::hint::black_box`] so it is not
    /// removed as a dead store; without `unsafe` volatile writes that is a
    /// strong hint to the compiler, not a guarantee.
    ///
    /// # Examples
    ///
    /// ```
    /// use compress_lib::deflate::Inflater;
    ///
    /// let mut inflater = Inflater::new();
    /// let mut out = Vec::new();
    /// inflater.finish_vec(&[0xCB, 0x48, 0xCD, 0xC9, 0xC9, 0x07, 0x00], &mut out)?;
    /// inflater.wipe();
    /// assert_eq!(inflater.total_out(), 0);
    /// # Ok::<(), compress_lib::Error>(())
    /// ```
    pub fn wipe(&mut self) {
        crate::wipe_vec(&mut self.ring);
        *self.tables = Tables::new();
        core::hint::black_box(&*self.tables);
        self.pending_len = 0;
        self.pending_dist = 0;
        self.reset();
    }

    /// After `Done`, starts a new DEFLATE stream that keeps the window, so
    /// the next stream may refer back into this one. This is RFC 7692 context
    /// takeover after a message that ended with a final block.
    ///
    /// # Errors
    ///
    /// [`ErrorKind::InvalidState`] before the current stream is done.
    ///
    /// # Examples
    ///
    /// ```
    /// use compress_lib::deflate::Inflater;
    ///
    /// let mut inflater = Inflater::new();
    /// let mut out = Vec::new();
    /// inflater.finish_vec(&[0x03, 0x00], &mut out)?;
    /// inflater.next_stream()?;
    /// inflater.finish_vec(&[0x03, 0x00], &mut out)?;
    /// # Ok::<(), compress_lib::Error>(())
    /// ```
    pub fn next_stream(&mut self) -> Result<(), Error> {
        if self.state != State::Done {
            return Err(Error::config(ErrorKind::InvalidState));
        }
        self.state = State::Header;
        self.final_block = false;
        self.bitbuf = 0;
        self.bitcnt = 0;
        Ok(())
    }

    /// Presets the history with `dictionary` (its last `2^window_bits`
    /// bytes), for streams compressed with the same preset dictionary. Only
    /// before any input.
    ///
    /// # Errors
    ///
    /// [`ErrorKind::InvalidState`] after input has been given.
    ///
    /// # Examples
    ///
    /// ```
    /// use compress_lib::deflate::Inflater;
    ///
    /// let mut inflater = Inflater::new();
    /// inflater.set_dictionary(b"common prefix")?;
    /// # Ok::<(), compress_lib::Error>(())
    /// ```
    pub fn set_dictionary(&mut self, dictionary: &[u8]) -> Result<(), Error> {
        if self.total_in != 0 || self.state != State::Header || self.failed.is_some() {
            return Err(Error::config(ErrorKind::InvalidState));
        }
        if self.ring.is_empty() {
            self.ring = alloc::vec![0; 1 << self.window_bits];
            self.keep_history = true;
        }
        self.save_history(dictionary);
        Ok(())
    }

    /// Decompresses as much of `input` into `output` as fits. More input may
    /// follow in later calls.
    ///
    /// Returns how much input was consumed, how much output was produced,
    /// and the [`Status`]: `NeedsInput` (all input consumed, the stream goes
    /// on), `NeedsOutput` (output is full; call again with more room and the
    /// rest of the input), or `Done` (the final block ended; `consumed`
    /// stops at the stream's last byte, so trailing data stays with the
    /// caller).
    ///
    /// # Errors
    ///
    /// Any decoding error of [`ErrorKind`], or `OutputLimit` / `RatioLimit`
    /// when a limit stops the stream. After an error every call returns the
    /// same error until [`reset`](Inflater::reset).
    ///
    /// # Examples
    ///
    /// ```
    /// use compress_lib::deflate::{Inflater, Status};
    ///
    /// let compressed = [0xCB, 0x48, 0xCD, 0xC9, 0xC9, 0x07, 0x00];
    /// let mut inflater = Inflater::new();
    /// let mut out = Vec::new();
    /// let mut buf = [0u8; 2]; // tiny on purpose
    /// let mut input = &compressed[..];
    /// loop {
    ///     let p = inflater.decompress(input, &mut buf)?;
    ///     input = &input[p.consumed..];
    ///     out.extend_from_slice(&buf[..p.produced]);
    ///     if p.status == Status::Done {
    ///         break;
    ///     }
    /// }
    /// assert_eq!(out, b"hello");
    /// # Ok::<(), compress_lib::Error>(())
    /// ```
    pub fn decompress(&mut self, input: &[u8], output: &mut [u8]) -> Result<Progress, Error> {
        self.run_slice(input, output, false)
    }

    /// As [`decompress`](Inflater::decompress), with `input` the last of the
    /// stream: if the stream is unfinished once input runs out, the result
    /// is `UnexpectedEnd` (unless output was full first, which is
    /// `NeedsOutput`).
    ///
    /// # Errors
    ///
    /// As `decompress`, plus [`ErrorKind::UnexpectedEnd`].
    ///
    /// # Examples
    ///
    /// ```
    /// use compress_lib::{ErrorKind, deflate::Inflater};
    ///
    /// let truncated = [0xCB, 0x48, 0xCD];
    /// let mut out = [0u8; 16];
    /// let err = Inflater::new().finish(&truncated, &mut out).unwrap_err();
    /// assert_eq!(err.kind(), ErrorKind::UnexpectedEnd);
    /// ```
    pub fn finish(&mut self, input: &[u8], output: &mut [u8]) -> Result<Progress, Error> {
        self.run_slice(input, output, true)
    }

    /// Decompresses `input`, appending to `out`, growing it as needed but
    /// never past the output cap. Growth doubles what this call has
    /// produced (at least 64 KiB at a time), so peak memory is at most 1.5
    /// times `max_output` while a reallocation holds the old buffer and the
    /// new one. Returns `NeedsInput` or `Done`.
    ///
    /// # Errors
    ///
    /// As [`decompress`](Inflater::decompress), plus
    /// [`ErrorKind::AllocationFailed`] when the vector cannot grow.
    ///
    /// # Examples
    ///
    /// ```
    /// use compress_lib::deflate::{Inflater, Status};
    ///
    /// let mut out = Vec::new();
    /// let p = Inflater::new().decompress_vec(&[0xCB, 0x48, 0xCD, 0xC9, 0xC9, 0x07, 0x00], &mut out)?;
    /// assert_eq!(p.status, Status::Done);
    /// assert_eq!(out, b"hello");
    /// # Ok::<(), compress_lib::Error>(())
    /// ```
    pub fn decompress_vec(&mut self, input: &[u8], out: &mut Vec<u8>) -> Result<Progress, Error> {
        self.run_vec(input, out, false)
    }

    /// As [`decompress_vec`](Inflater::decompress_vec), with `input` the last
    /// of the stream: an unfinished stream is `UnexpectedEnd`.
    ///
    /// # Errors
    ///
    /// As `decompress_vec`, plus [`ErrorKind::UnexpectedEnd`].
    ///
    /// # Examples
    ///
    /// ```
    /// use compress_lib::deflate::Inflater;
    ///
    /// let mut out = Vec::new();
    /// Inflater::new().finish_vec(&[0x03, 0x00], &mut out)?;
    /// assert!(out.is_empty());
    /// # Ok::<(), compress_lib::Error>(())
    /// ```
    pub fn finish_vec(&mut self, input: &[u8], out: &mut Vec<u8>) -> Result<Progress, Error> {
        self.run_vec(input, out, true)
    }

    fn run_slice(
        &mut self,
        input: &[u8],
        output: &mut [u8],
        last: bool,
    ) -> Result<Progress, Error> {
        let (consumed, produced, status) = self.run(input, output, 0, last)?;
        if self.keep_history {
            self.save_history(&output[..produced]);
        }
        Ok(Progress::new(consumed, produced, status))
    }

    fn run_vec(&mut self, input: &[u8], out: &mut Vec<u8>, last: bool) -> Result<Progress, Error> {
        let start = out.len();
        let p = self.run_vec_from(input, out, start, last)?;
        if self.keep_history {
            self.save_history(&out[start..]);
        }
        Ok(p)
    }

    /// Decodes into `out`, treating `out[history_start..]` as this stream's
    /// output so far (it is the history for back-references). The ring is
    /// not updated; the caller records the history when it is done with it.
    pub(crate) fn decompress_into(
        &mut self,
        input: &[u8],
        out: &mut Vec<u8>,
        history_start: usize,
        last: bool,
    ) -> Result<Progress, Error> {
        self.run_vec_from(input, out, history_start, last)
    }

    /// Whether the decoder is waiting for a block header at a byte boundary,
    /// with no bits left over: where a sync-flushed message must end.
    pub(crate) fn at_block_boundary(&self) -> bool {
        self.state == State::Header && self.bitcnt == 0
    }

    /// Records `data` as the newest history (when a window is kept).
    pub(crate) fn remember(&mut self, data: &[u8]) {
        if self.keep_history {
            self.save_history(data);
        }
    }

    /// Replaces the output cap.
    pub(crate) fn set_max_output(&mut self, max: u64) {
        self.limits = self.limits.with_max_output(max);
    }

    /// Counts `taken` input and `produced` output bytes from before this
    /// stream (earlier members of the same gzip stream) towards the ratio
    /// bound, so the ratio and its floor cover the container as a whole.
    /// Cleared by [`Inflater::reset`].
    pub(crate) fn set_ratio_base(&mut self, taken: u64, produced: u64) {
        self.ratio_base_in = taken;
        self.ratio_base_out = produced;
    }

    fn run_vec_from(
        &mut self,
        input: &[u8],
        out: &mut Vec<u8>,
        start: usize,
        last: bool,
    ) -> Result<Progress, Error> {
        let first = out.len();
        let mut consumed = 0;
        // Room offered per step: a first guess from this call's input (four
        // times it, 256 B to 64 KiB), doubling only after the step filled
        // up, never past the cap. Each call zero-fills O(input + output)
        // bytes, never the stream's earlier output (T-16).
        let mut step = input.len().saturating_mul(4).clamp(256, VEC_CHUNK);
        let status = loop {
            let have = out.len();
            let made = have - start;
            let cap_room = usize::try_from(self.limits.max_output().saturating_sub(self.total_out))
                .unwrap_or(usize::MAX);
            let mut want = cap_room.min(step);
            let spare = out.capacity() - have;
            if spare < want && spare >= want.min(VEC_MIN_SPARE) {
                // Use what is already reserved before growing: a stream that
                // fits its reservation would otherwise reallocate and copy
                // everything for its last few bytes.
                want = spare;
            }
            if spare < want {
                // Grow the vector geometrically (as `Vec` does) so that many
                // small calls reallocate O(log n) times, but never past what
                // the cap still allows this stream to produce. Once the next
                // doubling would pass the cap, go straight to it: a last
                // small step would hold two buffers near the cap at once
                // (T-1).
                let limit = have.saturating_add(cap_room);
                let mut target = (have + want).max(out.capacity().saturating_mul(2));
                if target.saturating_mul(2) > limit {
                    target = limit;
                }
                if out.try_reserve_exact(target - have).is_err() {
                    return Err(self.fail(ErrorKind::AllocationFailed, 0));
                }
            }
            #[cfg(test)]
            tests::count_zero_fill(want);
            out.resize(have + want, 0);
            match self.run(&input[consumed..], &mut out[start..], made, last) {
                Ok((c, p, status)) => {
                    out.truncate(have + p);
                    consumed += c;
                    if status != Status::NeedsOutput {
                        break status;
                    }
                    step = step.saturating_mul(2);
                }
                Err(e) => {
                    out.truncate(have);
                    return Err(e);
                }
            }
        };
        Ok(Progress::new(consumed, out.len() - first, status))
    }

    /// Records `newest` (this call's output) in the history ring.
    fn save_history(&mut self, newest: &[u8]) {
        let size = self.ring.len();
        if size == 0 || newest.is_empty() {
            return;
        }
        if newest.len() >= size {
            self.ring.copy_from_slice(&newest[newest.len() - size..]);
            self.wpos = 0;
            self.whave = size;
            return;
        }
        let first = (size - self.wpos).min(newest.len());
        self.ring[self.wpos..self.wpos + first].copy_from_slice(&newest[..first]);
        let rest = newest.len() - first;
        self.ring[..rest].copy_from_slice(&newest[first..]);
        self.wpos = (self.wpos + newest.len()) & (size - 1);
        self.whave = (self.whave + newest.len()).min(size);
    }

    /// Marks the inflater failed with `kind` at the current input position.
    fn fail(&mut self, kind: ErrorKind, ip: usize) -> Error {
        let offset = (self.total_in + ip as u64).saturating_sub(u64::from(self.bitcnt / 8));
        let e = Error::new(kind, offset);
        self.failed = Some(e);
        e
    }

    /// Decodes into `buf[op_start..]`, with `buf[..op_start]` already-valid
    /// output of this stream (history). Returns (consumed, produced, status).
    fn run(
        &mut self,
        input: &[u8],
        buf: &mut [u8],
        op_start: usize,
        last: bool,
    ) -> Result<(usize, usize, Status), Error> {
        if let Some(e) = self.failed {
            return Err(e);
        }
        let mut ip = 0;
        let mut op = op_start;
        let stop = loop {
            let cap_room = self
                .limits
                .max_output()
                .saturating_sub(self.total_out + (op - op_start) as u64);
            let cap_end = op
                + usize::try_from(cap_room)
                    .unwrap_or(usize::MAX)
                    .min(buf.len() - op);
            let mut end = cap_end;
            if self.limits.max_ratio() != 0 {
                let taken = self.ratio_base_in + self.total_in + ip as u64;
                let allowed = taken
                    .saturating_mul(u64::from(self.limits.max_ratio()))
                    .max(self.limits.ratio_floor());
                let ratio_room = allowed
                    .saturating_sub(self.ratio_base_out + self.total_out + (op - op_start) as u64);
                end = end.min(op + usize::try_from(ratio_room).unwrap_or(usize::MAX));
            }
            let before = (ip, op);
            let stop = match self.step(input, &mut ip, buf, &mut op, end) {
                Ok(stop) => stop,
                Err(kind) => {
                    self.total_out += (op - op_start) as u64;
                    return Err(self.fail(kind, ip));
                }
            };
            if stop != Stop::NeedOutput {
                break stop;
            }
            // More output is pending: past the cap that is the end, whatever
            // room the caller gave.
            if op == cap_end && cap_room == (op - before.1) as u64 {
                self.total_out += (op - op_start) as u64;
                return Err(self.fail(ErrorKind::OutputLimit, ip));
            }
            if op == buf.len() {
                break stop;
            }
            // Stopped at the ratio bound. More input taken since raises it;
            // if nothing moved, the ratio is exceeded.
            if (ip, op) == before && op == end {
                self.total_out += (op - op_start) as u64;
                return Err(self.fail(ErrorKind::RatioLimit, ip));
            }
        };

        let produced = op - op_start;
        self.total_out += produced as u64;
        if stop == Stop::NeedInput && last {
            self.total_in += ip as u64;
            return Err(self.fail(ErrorKind::UnexpectedEnd, 0));
        }
        // Give back whole bytes the bit buffer read ahead, so `consumed`
        // stops at the last bit used (after `Done`, trailing data stays with
        // the caller). On `NeedsInput` every buffered bit belongs to the
        // pending symbol, and all input counts as consumed.
        if stop != Stop::NeedInput {
            let give = ((self.bitcnt / 8) as usize).min(ip);
            ip -= give;
            self.bitcnt -= 8 * give as u32;
        }
        self.bitbuf &= (1u64 << self.bitcnt) - 1;
        if stop == Stop::Done {
            self.bitbuf = 0;
            self.bitcnt = 0;
        }
        self.total_in += ip as u64;
        let status = match stop {
            Stop::Done => Status::Done,
            Stop::NeedInput => Status::NeedsInput,
            Stop::NeedOutput => Status::NeedsOutput,
        };
        Ok((ip, produced, status))
    }

    /// Pulls bytes into the bit buffer until it holds `n` bits. `false` when
    /// input runs out first.
    #[inline]
    fn need(&mut self, n: u32, input: &[u8], ip: &mut usize) -> bool {
        while self.bitcnt < n {
            if *ip >= input.len() {
                return false;
            }
            self.bitbuf |= u64::from(input[*ip]) << self.bitcnt;
            self.bitcnt += 8;
            *ip += 1;
        }
        true
    }

    #[inline]
    fn bits(&self, n: u32) -> u32 {
        (self.bitbuf & ((1u64 << n) - 1)) as u32
    }

    #[inline]
    fn consume(&mut self, n: u32) {
        self.bitbuf >>= n;
        self.bitcnt -= n;
    }

    /// Looks up the next symbol in `table`, pulling bytes one at a time
    /// until the entry's bits are buffered. Returns the entry with the bits
    /// to consume, or `None` when input runs out first (nothing consumed).
    fn peek(&mut self, table: Which, input: &[u8], ip: &mut usize) -> Option<(u32, u32)> {
        loop {
            let (t, root): (&[u32], u32) = match table {
                Which::Lit if self.fixed => (&FIXED_LIT, LIT_ROOT),
                Which::Lit => (&self.tables.lit, LIT_ROOT),
                Which::Dist if self.fixed => (&FIXED_DIST, DIST_ROOT),
                Which::Dist => (&self.tables.dist, DIST_ROOT),
                Which::Pre => (&self.tables.pre, PRE_ROOT),
            };
            let mut e = t[(self.bitbuf & ((1u64 << root) - 1)) as usize];
            let mut skip = 0;
            if e & SUBTABLE != 0 {
                let bits = (e >> 8) & 0xF;
                let index = ((self.bitbuf >> root) & ((1u64 << bits) - 1)) as usize;
                e = t[(e >> 16) as usize + index];
                skip = root;
            }
            let needed = skip + (e & 0xFF);
            if self.bitcnt >= needed {
                return Some((e, skip));
            }
            if *ip >= input.len() {
                return None;
            }
            self.bitbuf |= u64::from(input[*ip]) << self.bitcnt;
            self.bitcnt += 8;
            *ip += 1;
        }
    }

    /// The state that follows the end of a block.
    fn end_block(&mut self) -> State {
        if self.final_block {
            State::Done
        } else {
            State::Header
        }
    }

    /// Checks a distance against the window and the history available with
    /// `in_buf` bytes of this stream's output before the write position.
    #[inline]
    fn check_distance(&self, dist: usize, in_buf: usize) -> Result<(), ErrorKind> {
        if dist > (1usize << self.window_bits) || (dist > in_buf && dist - in_buf > self.whave) {
            Err(ErrorKind::DistanceTooFar)
        } else {
            Ok(())
        }
    }

    /// Copies `n` bytes of a match at `dist` to `buf[op..]`; the part before
    /// `buf[0]` comes from the history ring. The distance was checked.
    fn copy_match(&self, buf: &mut [u8], op: usize, dist: usize, n: usize) {
        if dist <= op {
            copy_forward(buf, op - dist, op, n);
            return;
        }
        let back = dist - op;
        let from_ring = back.min(n);
        let size = self.ring.len();
        let mut r = (self.wpos + size - back) & (size - 1);
        let mut o = op;
        let mut left = from_ring;
        while left > 0 {
            let run = left.min(size - r);
            buf[o..o + run].copy_from_slice(&self.ring[r..r + run]);
            o += run;
            left -= run;
            r = (r + run) & (size - 1);
        }
        if n > from_ring {
            // The rest of the match starts at buf[0].
            copy_forward(buf, 0, o, n - from_ring);
        }
    }

    /// Runs the state machine until it stops for input, output, or the end.
    fn step(
        &mut self,
        input: &[u8],
        ip: &mut usize,
        buf: &mut [u8],
        op: &mut usize,
        end: usize,
    ) -> Result<Stop, ErrorKind> {
        loop {
            match self.state {
                State::Header => {
                    if !self.need(3, input, ip) {
                        return Ok(Stop::NeedInput);
                    }
                    self.final_block = self.bits(1) == 1;
                    let kind = (self.bitbuf >> 1) & 3;
                    self.consume(3);
                    self.state = match kind {
                        0 => State::StoredLength,
                        1 => {
                            self.fixed = true;
                            State::Data
                        }
                        2 => State::Counts,
                        _ => return Err(ErrorKind::InvalidBlockType),
                    };
                }
                State::StoredLength => {
                    let pad = self.bitcnt % 8;
                    self.consume(pad);
                    if !self.need(32, input, ip) {
                        return Ok(Stop::NeedInput);
                    }
                    let len = self.bits(16);
                    let nlen = (self.bitbuf >> 16) as u32 & 0xFFFF;
                    if len != !nlen & 0xFFFF {
                        return Err(ErrorKind::InvalidStoredLength);
                    }
                    self.consume(32);
                    self.stored_left = len;
                    self.state = State::Stored;
                }
                State::Stored => {
                    while self.stored_left > 0 {
                        if *op >= end {
                            return Ok(Stop::NeedOutput);
                        }
                        if self.bitcnt >= 8 {
                            buf[*op] = self.bits(8) as u8;
                            self.consume(8);
                            *op += 1;
                            self.stored_left -= 1;
                        } else if *ip < input.len() {
                            let n = (self.stored_left as usize)
                                .min(input.len() - *ip)
                                .min(end - *op);
                            buf[*op..*op + n].copy_from_slice(&input[*ip..*ip + n]);
                            *op += n;
                            *ip += n;
                            self.stored_left -= n as u32;
                        } else {
                            return Ok(Stop::NeedInput);
                        }
                    }
                    self.state = self.end_block();
                }
                State::Counts => {
                    if !self.need(14, input, ip) {
                        return Ok(Stop::NeedInput);
                    }
                    self.hlit = self.bits(5) as u16 + 257;
                    self.hdist = (self.bitbuf >> 5) as u16 & 0x1F;
                    self.hdist += 1;
                    self.hclen = (self.bitbuf >> 10) as u16 & 0xF;
                    self.hclen += 4;
                    self.consume(14);
                    if self.hlit > 286 || self.hdist > 30 {
                        return Err(ErrorKind::InvalidCodeLengths);
                    }
                    self.tables.lens[..19].fill(0);
                    self.index = 0;
                    self.state = State::PrecodeLengths;
                }
                State::PrecodeLengths => {
                    while self.index < self.hclen {
                        if !self.need(3, input, ip) {
                            return Ok(Stop::NeedInput);
                        }
                        let slot = PRECODE_ORDER[self.index as usize] as usize;
                        self.tables.lens[slot] = self.bits(3) as u8;
                        self.consume(3);
                        self.index += 1;
                    }
                    let t = &mut *self.tables;
                    tables::build(
                        &t.lens[..19],
                        &mut t.pre,
                        PRE_ROOT,
                        Alphabet::Precode,
                        false,
                    )?;
                    self.index = 0;
                    self.state = State::CodeLengths;
                }
                State::CodeLengths => {
                    let total = self.hlit + self.hdist;
                    while self.index < total {
                        let Some((e, _)) = self.peek(Which::Pre, input, ip) else {
                            return Ok(Stop::NeedInput);
                        };
                        let symbol = e >> 16;
                        let len = e & 0xFF;
                        if symbol < 16 {
                            self.consume(len);
                            self.tables.lens[self.index as usize] = symbol as u8;
                            self.index += 1;
                            continue;
                        }
                        let (extra, base) = match symbol {
                            16 => (2, 3),
                            17 => (3, 3),
                            _ => (7, 11),
                        };
                        if !self.need(len + extra, input, ip) {
                            return Ok(Stop::NeedInput);
                        }
                        let repeat = base + ((self.bitbuf >> len) as u32 & ((1 << extra) - 1));
                        let value = if symbol == 16 {
                            if self.index == 0 {
                                return Err(ErrorKind::InvalidCodeLengths);
                            }
                            self.tables.lens[self.index as usize - 1]
                        } else {
                            0
                        };
                        if u32::from(self.index) + repeat > u32::from(total) {
                            return Err(ErrorKind::InvalidCodeLengths);
                        }
                        self.consume(len + extra);
                        let i = self.index as usize;
                        self.tables.lens[i..i + repeat as usize].fill(value);
                        self.index += repeat as u16;
                    }
                    let (hlit, hdist) = (self.hlit as usize, self.hdist as usize);
                    let t = &mut *self.tables;
                    if t.lens[256] == 0 {
                        return Err(ErrorKind::InvalidCodeLengths);
                    }
                    tables::build(
                        &t.lens[..hlit],
                        &mut t.lit,
                        LIT_ROOT,
                        Alphabet::LitLen,
                        true,
                    )?;
                    tables::build(
                        &t.lens[hlit..hlit + hdist],
                        &mut t.dist,
                        DIST_ROOT,
                        Alphabet::Dist,
                        true,
                    )?;
                    self.fixed = false;
                    self.state = State::Data;
                }
                State::Data => {
                    if input.len() - *ip >= FAST_IN && end - *op >= FAST_OUT {
                        self.fast(input, ip, buf, op, end)?;
                        continue;
                    }
                    let Some((e, skip)) = self.peek(Which::Lit, input, ip) else {
                        return Ok(Stop::NeedInput);
                    };
                    if e & LITERAL != 0 {
                        if *op >= end {
                            return Ok(Stop::NeedOutput);
                        }
                        self.consume(skip + (e & 0xFF));
                        buf[*op] = (e >> 16) as u8;
                        *op += 1;
                    } else if e & END_OF_BLOCK != 0 {
                        self.consume(skip + (e & 0xFF));
                        self.state = self.end_block();
                    } else if e & INVALID != 0 {
                        return Err(ErrorKind::InvalidSymbol);
                    } else {
                        let code = skip + ((e >> 8) & 0xF);
                        let extra = (e & 0xFF) - ((e >> 8) & 0xF);
                        let value = (self.bitbuf >> code) as u32 & ((1 << extra) - 1);
                        self.consume(skip + (e & 0xFF));
                        self.pending_len = (e >> 16) + value;
                        self.state = State::Distance;
                    }
                }
                State::Distance => {
                    let Some((e, skip)) = self.peek(Which::Dist, input, ip) else {
                        return Ok(Stop::NeedInput);
                    };
                    if e & INVALID != 0 {
                        return Err(ErrorKind::InvalidSymbol);
                    }
                    let code = skip + ((e >> 8) & 0xF);
                    let extra = (e & 0xFF) - ((e >> 8) & 0xF);
                    let value = (self.bitbuf >> code) as u32 & ((1 << extra) - 1);
                    let dist = (e >> 16) + value;
                    self.check_distance(dist as usize, *op)?;
                    self.consume(skip + (e & 0xFF));
                    self.pending_dist = dist;
                    self.state = State::Copy;
                }
                State::Copy => {
                    let n = (self.pending_len as usize).min(end - *op);
                    if n > 0 {
                        self.copy_match(buf, *op, self.pending_dist as usize, n);
                        *op += n;
                        self.pending_len -= n as u32;
                    }
                    if self.pending_len > 0 {
                        return Ok(Stop::NeedOutput);
                    }
                    self.state = State::Data;
                }
                State::Done => return Ok(Stop::Done),
            }
        }
    }

    /// The fast loop: decodes symbols while at least `FAST_IN` input bytes
    /// and `FAST_OUT` output bytes remain, with one eight-byte refill per
    /// symbol and no state saved between symbols. Returns at the end of the
    /// block or when the margins run out; the slow path does the rest.
    fn fast(
        &mut self,
        input: &[u8],
        ip: &mut usize,
        buf: &mut [u8],
        op: &mut usize,
        end: usize,
    ) -> Result<(), ErrorKind> {
        let (lit, dist): (&[u32], &[u32]) = if self.fixed {
            (&FIXED_LIT, &FIXED_DIST)
        } else {
            (&self.tables.lit, &self.tables.dist)
        };
        let window = 1usize << self.window_bits;
        let mut bitbuf = self.bitbuf;
        let mut bitcnt = self.bitcnt;
        let mut i = *ip;
        let mut o = *op;
        let result = loop {
            if input.len() - i < FAST_IN || end - o < FAST_OUT {
                break Ok(());
            }
            let mut word = [0u8; 8];
            word.copy_from_slice(&input[i..i + 8]);
            bitbuf |= u64::from_le_bytes(word) << bitcnt;
            i += ((63 - bitcnt) >> 3) as usize;
            bitcnt |= 56;

            let mut e = lit[(bitbuf & ((1 << LIT_ROOT) - 1)) as usize];
            if e & SUBTABLE != 0 {
                bitbuf >>= LIT_ROOT;
                bitcnt -= LIT_ROOT;
                e = lit[(e >> 16) as usize + (bitbuf & ((1u64 << ((e >> 8) & 0xF)) - 1)) as usize];
            }
            if e & LITERAL != 0 {
                buf[o] = (e >> 16) as u8;
                o += 1;
                bitbuf >>= e & 0xFF;
                bitcnt -= e & 0xFF;
                // A second literal often fits in the same refill.
                let e2 = lit[(bitbuf & ((1 << LIT_ROOT) - 1)) as usize];
                if e2 & LITERAL != 0 && bitcnt >= (e2 & 0xFF) {
                    buf[o] = (e2 >> 16) as u8;
                    o += 1;
                    bitbuf >>= e2 & 0xFF;
                    bitcnt -= e2 & 0xFF;
                }
                continue;
            }
            if e & (END_OF_BLOCK | INVALID) != 0 {
                if e & END_OF_BLOCK != 0 {
                    bitbuf >>= e & 0xFF;
                    bitcnt -= e & 0xFF;
                    self.state = if self.final_block {
                        State::Done
                    } else {
                        State::Header
                    };
                    break Ok(());
                }
                break Err(ErrorKind::InvalidSymbol);
            }
            let code = (e >> 8) & 0xF;
            let length = (e >> 16) as usize
                + ((bitbuf >> code) & ((1u64 << ((e & 0xFF) - code)) - 1)) as usize;
            bitbuf >>= e & 0xFF;
            bitcnt -= e & 0xFF;

            let mut d = dist[(bitbuf & ((1 << DIST_ROOT) - 1)) as usize];
            if d & SUBTABLE != 0 {
                bitbuf >>= DIST_ROOT;
                bitcnt -= DIST_ROOT;
                d = dist[(d >> 16) as usize + (bitbuf & ((1u64 << ((d >> 8) & 0xF)) - 1)) as usize];
            }
            if d & INVALID != 0 {
                break Err(ErrorKind::InvalidSymbol);
            }
            let code = (d >> 8) & 0xF;
            let distance = (d >> 16) as usize
                + ((bitbuf >> code) & ((1u64 << ((d & 0xFF) - code)) - 1)) as usize;
            bitbuf >>= d & 0xFF;
            bitcnt -= d & 0xFF;

            if distance > window || (distance > o && distance - o > self.whave) {
                break Err(ErrorKind::DistanceTooFar);
            }
            if distance <= o && distance >= 8 {
                // Eight bytes at a time; the source is always written before
                // it is read because the distance is at least eight, and the
                // over-copy stays inside the FAST_OUT margin.
                let mut s = o - distance;
                let stop = o + length;
                while o < stop {
                    buf.copy_within(s..s + 8, o);
                    s += 8;
                    o += 8;
                }
                o = stop;
            } else if distance == 1 && o >= 1 {
                // (At the start of this call's output a distance of 1 reaches
                // into the ring: the general copy below handles it.)
                let b = buf[o - 1];
                buf[o..o + length].fill(b);
                o += length;
            } else if distance <= o {
                // A period of 2 to 7 bytes. An eight-byte copy from one
                // period back is right for its first `distance` bytes, so
                // step by the period until a multiple of it of at least
                // eight bytes is written, then copy eight bytes at a time
                // from that far back. Writes stay inside the FAST_OUT margin.
                let period = distance * 8usize.div_ceil(distance);
                let stop = o + length;
                let mut w = o;
                while w < o + period - distance {
                    buf.copy_within(w - distance..w - distance + 8, w);
                    w += distance;
                }
                while w < stop {
                    buf.copy_within(w - period..w - period + 8, w);
                    w += 8;
                }
                o = stop;
            } else {
                self.copy_match(buf, o, distance, length);
                o += length;
            }
        };
        // The refill leaves read-ahead bits above `bitcnt`; the slow path
        // needs them clear.
        self.bitbuf = bitbuf & ((1u64 << bitcnt) - 1);
        self.bitcnt = bitcnt;
        *ip = i;
        *op = o;
        result
    }
}

/// Which table `peek` reads.
#[derive(Clone, Copy)]
enum Which {
    Lit,
    Dist,
    Pre,
}

#[cfg(test)]
mod tests;
