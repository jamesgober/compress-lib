//! The raw DEFLATE (RFC 1951) encoder.
//!
//! Input is copied into a sliding window; a hash of three bytes indexes
//! chains of earlier positions; greedy (levels 1 to 3) or lazy (4 to 9)
//! matching turns the window into literal and match symbols; every 16,384
//! symbols (fewer for small windows) a block is written as stored, fixed, or
//! dynamic Huffman, whichever is smallest. Output waits in a pending buffer
//! of at most one block until the caller drains it.

use alloc::{vec, vec::Vec};

use super::huffman;
use crate::{Error, ErrorKind, Level, Progress, Status};

const MIN_MATCH: usize = 3;
const MAX_MATCH: usize = 258;
/// Lookahead kept before matching at a position, so a full match and the
/// hash of the next position always fit.
const MIN_LOOKAHEAD: usize = MAX_MATCH + MIN_MATCH + 1;
/// A match of length 3 further back than this costs more than three
/// literals (zlib's TOO_FAR).
const TOO_FAR: usize = 4096;
/// Level 0 writes a stored block once this much input is waiting (or half
/// the buffer, for small windows).
const STORED_CHUNK: usize = 16 * 1024;

const LENGTH_BASE: [u16; 29] = [
    3, 4, 5, 6, 7, 8, 9, 10, 11, 13, 15, 17, 19, 23, 27, 31, 35, 43, 51, 59, 67, 83, 99, 115, 131,
    163, 195, 227, 258,
];
const LENGTH_EXTRA: [u8; 29] = [
    0, 0, 0, 0, 0, 0, 0, 0, 1, 1, 1, 1, 2, 2, 2, 2, 3, 3, 3, 3, 4, 4, 4, 4, 5, 5, 5, 5, 0,
];
const DIST_BASE: [u16; 30] = [
    1, 2, 3, 4, 5, 7, 9, 13, 17, 25, 33, 49, 65, 97, 129, 193, 257, 385, 513, 769, 1025, 1537,
    2049, 3073, 4097, 6145, 8193, 12289, 16385, 24577,
];
const DIST_EXTRA: [u8; 30] = [
    0, 0, 0, 0, 1, 1, 2, 2, 3, 3, 4, 4, 5, 5, 6, 6, 7, 7, 8, 8, 9, 9, 10, 10, 11, 11, 12, 12, 13,
    13,
];
const PRECODE_ORDER: [usize; 19] = [
    16, 17, 18, 0, 8, 7, 9, 6, 10, 5, 11, 4, 12, 3, 13, 2, 14, 1, 15,
];

/// `LENGTH_CODE[len - 3]`: the length symbol index (0 to 28).
static LENGTH_CODE: [u8; 256] = {
    let mut t = [0u8; 256];
    let mut code = 0;
    while code < 28 {
        let mut i = 0;
        while i < (1 << LENGTH_EXTRA[code]) {
            t[LENGTH_BASE[code] as usize - 3 + i] = code as u8;
            i += 1;
        }
        code += 1;
    }
    // 258 has its own symbol; 257 - 3 = 254 stays in code 27.
    t[255] = 28;
    t
};

/// Distance symbol for `dist - 1` below 256, and for `256 + ((dist - 1) >> 7)`
/// above (zlib's two-part table).
static DIST_CODE: [u8; 512] = {
    let mut t = [0u8; 512];
    let mut code = 0;
    while code < 30 {
        let base = DIST_BASE[code] as usize - 1;
        let count = 1usize << DIST_EXTRA[code];
        let mut i = 0;
        while i < count {
            let d = base + i;
            if d < 256 {
                t[d] = code as u8;
            } else {
                t[256 + (d >> 7)] = code as u8;
            }
            i += 1;
        }
        code += 1;
    }
    t
};

#[inline]
fn dist_code(dist: usize) -> usize {
    let d = dist - 1;
    if d < 256 {
        DIST_CODE[d] as usize
    } else {
        DIST_CODE[256 + (d >> 7)] as usize
    }
}

/// Fixed literal/length code lengths.
const fn fixed_lit_lens() -> [u8; 288] {
    let mut l = [0u8; 288];
    let mut i = 0;
    while i < 288 {
        l[i] = if i < 144 {
            8
        } else if i < 256 {
            9
        } else if i < 280 {
            7
        } else {
            8
        };
        i += 1;
    }
    l
}
static FIXED_LIT_LENS: [u8; 288] = fixed_lit_lens();

/// Match-finder settings per level (zlib's configuration table).
#[derive(Clone, Copy)]
struct Params {
    /// Shorten the chain search once a match this long is found.
    good: usize,
    /// Greedy levels: insert hashes inside matches up to this length. Lazy
    /// levels: do not look for a better match once one this long is found.
    lazy: usize,
    /// Stop searching at a match this long.
    nice: usize,
    /// Chain positions searched per match.
    chain: usize,
    lazy_matching: bool,
}

const fn params(level: u8) -> Params {
    let (good, lazy, nice, chain, lazy_matching) = match level {
        1 => (4, 4, 8, 4, false),
        2 => (4, 5, 16, 8, false),
        3 => (4, 6, 32, 32, false),
        4 => (4, 4, 16, 16, true),
        5 => (8, 16, 32, 32, true),
        6 => (8, 16, 128, 128, true),
        7 => (8, 32, 128, 256, true),
        8 => (32, 128, 258, 1024, true),
        9 => (32, 258, 258, 4096, true),
        _ => (0, 0, 0, 0, false),
    };
    Params {
        good,
        lazy,
        nice,
        chain,
        lazy_matching,
    }
}

/// How far a call should go before returning.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Flush {
    /// Compress as input allows; output may lag behind input.
    None,
    /// Write out everything so far, end on a byte boundary with an empty
    /// stored block (`00 00 FF FF`). History is kept. WebSocket
    /// permessage-deflate ends every message this way.
    Sync,
    /// As `Sync`, and forget history: later data never refers back across
    /// this point, so a decoder can start here.
    Full,
    /// Write out everything and end the stream with a final block.
    Finish,
}

/// Scratch for building one block's codes (about 3 KiB, boxed once).
struct BlockScratch {
    lit_freq: [u32; 286],
    dist_freq: [u32; 30],
    lit_lens: [u8; 286],
    dist_lens: [u8; 30],
    lit_codes: [u16; 288],
    dist_codes: [u16; 30],
    /// Run-length coded code lengths: (symbol, extra value).
    rle: [(u8, u8); 316],
    rle_len: usize,
    pre_freq: [u32; 19],
    pre_lens: [u8; 19],
    pre_codes: [u16; 19],
}

/// A streaming raw DEFLATE encoder (RFC 1951).
///
/// Push input, pull output, with a [`Flush`] choosing how far each call
/// goes. Memory is fixed when the deflater is built (section 5.6 of the
/// plan): at window bits 15 and level 6, a 64 KiB window, 128 KiB of hash
/// chains, 64 KiB of symbols, and up to one block of pending output.
///
/// # Examples
///
/// ```
/// use compress_lib::{Level, deflate::{self, Deflater, Flush}};
///
/// let mut deflater = Deflater::new(Level::DEFAULT);
/// let mut packed = Vec::new();
/// deflater.compress_vec(b"hello, hello, hello", &mut packed, Flush::Finish)?;
/// assert_eq!(deflate::decompress(&packed)?, b"hello, hello, hello");
/// # Ok::<(), compress_lib::Error>(())
/// ```
pub struct Deflater {
    level: u8,
    params: Params,
    window_bits: u8,
    /// Window buffer of `wsize * span` bytes; positions are indices into it.
    win: Vec<u8>,
    wsize: usize,
    /// History always kept before `strstart`, and so the longest distance
    /// used: the whole window, or 262 bytes less when the buffer is only two
    /// windows (zlib's MAX_DIST), so positions fit in 16 bits.
    keep: usize,
    strstart: usize,
    lookahead: usize,
    /// Start of the current block's input in `win` (negative once slid out).
    block_start: isize,
    hash_bits: u32,
    head: Vec<u16>,
    prev: Vec<u16>,
    match_available: bool,
    match_length: usize,
    match_start: usize,
    prev_length: usize,
    prev_match: usize,
    /// Symbols of the current block: `dist << 16 | litlen`, where a literal
    /// has `dist == 0` and a match stores `length - 3`.
    syms: Vec<u32>,
    sym_cap: usize,
    scratch: alloc::boxed::Box<BlockScratch>,
    pending: Vec<u8>,
    pending_pos: usize,
    bitbuf: u64,
    bitcnt: u32,
    total_in: u64,
    total_out: u64,
    /// The flush asked for has been written into `pending`.
    flush_written: bool,
    finished: bool,
}

impl core::fmt::Debug for Deflater {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Deflater")
            .field("level", &self.level)
            .field("window_bits", &self.window_bits)
            .field("total_in", &self.total_in)
            .field("total_out", &self.total_out)
            .field("finished", &self.finished)
            .finish_non_exhaustive()
    }
}

impl Default for Deflater {
    fn default() -> Self {
        Self::new(Level::DEFAULT)
    }
}

impl Deflater {
    /// A deflater at `level` with window bits 15.
    ///
    /// # Examples
    ///
    /// ```
    /// use compress_lib::{Level, deflate::Deflater};
    ///
    /// let deflater = Deflater::new(Level::FAST);
    /// assert_eq!(deflater.level(), Level::FAST);
    /// ```
    #[must_use]
    pub fn new(level: Level) -> Self {
        Self::build(level.get(), 15)
    }

    fn build(level: u8, window_bits: u8) -> Self {
        let wsize = 1usize << window_bits;
        // At least 8 KiB of buffer, so small windows still read input in
        // useful pieces; always a whole number of windows (the hash chains
        // are indexed modulo the window).
        let span = (8192 / wsize).max(2);
        let hash_bits = u32::from(window_bits + 1).clamp(9, 15);
        let sym_cap = (1usize << (window_bits + 1)).clamp(1024, 16_384);
        Self {
            level,
            params: params(level),
            window_bits,
            win: vec![0; wsize * span],
            wsize,
            keep: if span >= 3 {
                wsize
            } else {
                wsize - MIN_LOOKAHEAD
            },
            strstart: 0,
            lookahead: 0,
            block_start: 0,
            hash_bits,
            head: vec![0; 1 << hash_bits],
            prev: vec![0; wsize],
            match_available: false,
            match_length: MIN_MATCH - 1,
            match_start: 0,
            prev_length: MIN_MATCH - 1,
            prev_match: 0,
            syms: Vec::with_capacity(sym_cap),
            sym_cap,
            scratch: alloc::boxed::Box::new(BlockScratch {
                lit_freq: [0; 286],
                dist_freq: [0; 30],
                lit_lens: [0; 286],
                dist_lens: [0; 30],
                lit_codes: [0; 288],
                dist_codes: [0; 30],
                rle: [(0, 0); 316],
                rle_len: 0,
                pre_freq: [0; 19],
                pre_lens: [0; 19],
                pre_codes: [0; 19],
            }),
            // One block is at most about four bytes per symbol (fixed codes,
            // longest extra bits), or the stored form of its input.
            pending: Vec::with_capacity(sym_cap * 4 + wsize * span + 64),
            pending_pos: 0,
            bitbuf: 0,
            bitcnt: 0,
            total_in: 0,
            total_out: 0,
            flush_written: false,
            finished: false,
        }
    }

    /// Sets the window to `2^bits` bytes, 8 to 15: no match distance is ever
    /// longer (RFC 7692 negotiates these sizes; 8 is a real 256-byte
    /// window). Smaller windows also use smaller hash and symbol tables.
    ///
    /// # Errors
    ///
    /// [`ErrorKind::InvalidWindowBits`] outside 8 to 15.
    ///
    /// # Examples
    ///
    /// ```
    /// use compress_lib::{Level, deflate::Deflater};
    ///
    /// let small = Deflater::new(Level::FAST).with_window_bits(9)?;
    /// assert!(small.memory_in_use() < Deflater::new(Level::FAST).memory_in_use());
    /// assert!(Deflater::new(Level::FAST).with_window_bits(7).is_err());
    /// # Ok::<(), compress_lib::Error>(())
    /// ```
    pub fn with_window_bits(self, bits: u8) -> Result<Self, Error> {
        if !(8..=15).contains(&bits) {
            return Err(Error::config(ErrorKind::InvalidWindowBits));
        }
        Ok(Self::build(self.level, bits))
    }

    /// The level in force.
    ///
    /// # Examples
    ///
    /// ```
    /// use compress_lib::{Level, deflate::Deflater};
    ///
    /// assert_eq!(Deflater::default().level(), Level::DEFAULT);
    /// ```
    #[must_use]
    pub fn level(&self) -> Level {
        Level::new(self.level).unwrap_or(Level::DEFAULT)
    }

    /// The window size in bits.
    ///
    /// # Examples
    ///
    /// ```
    /// assert_eq!(compress_lib::deflate::Deflater::default().window_bits(), 15);
    /// ```
    #[must_use]
    pub fn window_bits(&self) -> u8 {
        self.window_bits
    }

    /// Changes the level for data not yet compressed. The current block is
    /// ended first when the change switches to or from level 0.
    ///
    /// # Examples
    ///
    /// ```
    /// use compress_lib::{Level, deflate::{self, Deflater, Flush}};
    ///
    /// let mut deflater = Deflater::new(Level::BEST);
    /// let mut out = Vec::new();
    /// deflater.compress_vec(b"header ", &mut out, Flush::None)?;
    /// deflater.set_level(Level::NONE);
    /// deflater.compress_vec(b"body", &mut out, Flush::Finish)?;
    /// assert_eq!(deflate::decompress(&out)?, b"header body");
    /// # Ok::<(), compress_lib::Error>(())
    /// ```
    pub fn set_level(&mut self, level: Level) {
        let level = level.get();
        let next = params(level);
        if (level == 0) != (self.level == 0) {
            // Finish the input waiting under the old level first.
            self.process(true);
            self.end_block(false);
        } else if self.params.lazy_matching && !next.lazy_matching && self.match_available {
            // Lazy matching holds the byte before `strstart` back; greedy
            // matching does not know about it.
            self.literal(self.win[self.strstart - 1]);
            self.match_available = false;
            self.block_full();
        }
        // No pending match crosses the change.
        self.match_length = MIN_MATCH - 1;
        self.prev_length = MIN_MATCH - 1;
        self.level = level;
        self.params = next;
    }

    /// Uncompressed bytes taken since the deflater was built or reset.
    ///
    /// # Examples
    ///
    /// ```
    /// use compress_lib::deflate::{Deflater, Flush};
    ///
    /// let mut d = Deflater::default();
    /// d.compress_vec(b"abc", &mut Vec::new(), Flush::Finish)?;
    /// assert_eq!(d.total_in(), 3);
    /// # Ok::<(), compress_lib::Error>(())
    /// ```
    #[must_use]
    pub fn total_in(&self) -> u64 {
        self.total_in
    }

    /// Compressed bytes handed out since the deflater was built or reset.
    ///
    /// # Examples
    ///
    /// ```
    /// use compress_lib::deflate::{Deflater, Flush};
    ///
    /// let mut d = Deflater::default();
    /// let mut out = Vec::new();
    /// d.compress_vec(b"", &mut out, Flush::Finish)?;
    /// assert_eq!(d.total_out(), out.len() as u64);
    /// # Ok::<(), compress_lib::Error>(())
    /// ```
    #[must_use]
    pub fn total_out(&self) -> u64 {
        self.total_out
    }

    /// Heap bytes held: window, hash chains, symbols, pending output, and
    /// block scratch.
    ///
    /// # Examples
    ///
    /// ```
    /// let d = compress_lib::deflate::Deflater::default();
    /// assert!(d.memory_in_use() > 64 * 1024);
    /// ```
    #[must_use]
    pub fn memory_in_use(&self) -> usize {
        self.win.capacity()
            + 2 * (self.head.capacity() + self.prev.capacity())
            + 4 * self.syms.capacity()
            + self.pending.capacity()
            + core::mem::size_of::<BlockScratch>()
    }

    /// Starts a new, unrelated stream, keeping the allocations.
    ///
    /// # Examples
    ///
    /// ```
    /// use compress_lib::deflate::{self, Deflater, Flush};
    ///
    /// let mut d = Deflater::default();
    /// let mut a = Vec::new();
    /// d.compress_vec(b"first", &mut a, Flush::Finish)?;
    /// d.reset();
    /// let mut b = Vec::new();
    /// d.compress_vec(b"second", &mut b, Flush::Finish)?;
    /// assert_eq!(deflate::decompress(&b)?, b"second");
    /// # Ok::<(), compress_lib::Error>(())
    /// ```
    pub fn reset(&mut self) {
        self.strstart = 0;
        self.lookahead = 0;
        self.block_start = 0;
        self.head.fill(0);
        self.match_available = false;
        self.match_length = MIN_MATCH - 1;
        self.prev_length = MIN_MATCH - 1;
        self.syms.clear();
        self.pending.clear();
        self.pending_pos = 0;
        self.bitbuf = 0;
        self.bitcnt = 0;
        self.total_in = 0;
        self.total_out = 0;
        self.flush_written = false;
        self.finished = false;
    }

    /// Presets the history with `dictionary` (its last `2^window_bits`
    /// bytes): matches may refer into it. The decoder must be given the same
    /// dictionary. Only before any input.
    ///
    /// # Errors
    ///
    /// [`ErrorKind::InvalidState`] after input has been given.
    ///
    /// # Examples
    ///
    /// ```
    /// use compress_lib::deflate::{Deflater, Flush, Inflater};
    ///
    /// let dict = b"{\"type\":\"update\",\"value\":";
    /// let mut d = Deflater::default();
    /// d.set_dictionary(dict)?;
    /// let mut packed = Vec::new();
    /// d.compress_vec(b"{\"type\":\"update\",\"value\":42}", &mut packed, Flush::Finish)?;
    /// let mut i = Inflater::new();
    /// i.set_dictionary(dict)?;
    /// let mut out = Vec::new();
    /// i.finish_vec(&packed, &mut out)?;
    /// assert_eq!(out, b"{\"type\":\"update\",\"value\":42}");
    /// # Ok::<(), compress_lib::Error>(())
    /// ```
    pub fn set_dictionary(&mut self, dictionary: &[u8]) -> Result<(), Error> {
        if self.total_in != 0 || self.lookahead != 0 || self.strstart != 0 {
            return Err(Error::config(ErrorKind::InvalidState));
        }
        let take = dictionary.len().min(self.wsize);
        let dict = &dictionary[dictionary.len() - take..];
        // Position 0 is the hash chains' "none", so the dictionary starts at 1.
        self.win[1..1 + take].copy_from_slice(dict);
        self.strstart = 1;
        self.lookahead = take;
        if self.level != 0 {
            let end = 1 + take;
            let mut pos = 1;
            while pos + MIN_MATCH <= end {
                self.insert(pos);
                pos += 1;
            }
        }
        self.strstart = 1 + take;
        self.lookahead = 0;
        self.block_start = self.strstart as isize;
        Ok(())
    }

    /// Compresses as much of `input` as the window takes and writes
    /// compressed bytes to `output`.
    ///
    /// With `Flush::None` the result is `NeedsInput` once all input is taken
    /// (output may lag behind), or `NeedsOutput` when `output` is full.
    /// With a flush, `Done` means all input is taken and the flush point is
    /// completely in `output`; on `NeedsOutput`, call again with the same
    /// flush and the input not yet consumed.
    ///
    /// # Errors
    ///
    /// [`ErrorKind::InvalidState`] when input is given after `Finish`
    /// completed (call [`reset`](Deflater::reset) for a new stream).
    ///
    /// # Examples
    ///
    /// ```
    /// use compress_lib::deflate::{self, Deflater, Flush, Status};
    ///
    /// let mut d = Deflater::default();
    /// let mut out = [0u8; 256];
    /// let p = d.compress(b"some data", &mut out, Flush::Finish)?;
    /// assert_eq!(p.status, Status::Done);
    /// assert_eq!(deflate::decompress(&out[..p.produced])?, b"some data");
    /// # Ok::<(), compress_lib::Error>(())
    /// ```
    pub fn compress(
        &mut self,
        input: &[u8],
        output: &mut [u8],
        flush: Flush,
    ) -> Result<Progress, Error> {
        let mut consumed = 0;
        let mut produced = 0;
        if self.finished {
            if !input.is_empty() {
                return Err(Error::config(ErrorKind::InvalidState));
            }
            self.drain(output, &mut produced);
            let status = if self.pending_left() == 0 {
                Status::Done
            } else {
                Status::NeedsOutput
            };
            return Ok(Progress::new(0, produced, status));
        }
        loop {
            self.drain(output, &mut produced);
            if self.pending_left() > 0 {
                return Ok(Progress::new(consumed, produced, Status::NeedsOutput));
            }
            if self.flush_written && consumed == input.len() {
                self.flush_written = false;
                if flush == Flush::Finish {
                    self.finished = true;
                }
                return Ok(Progress::new(consumed, produced, Status::Done));
            }
            self.flush_written = false;
            let before = (consumed, self.strstart, self.lookahead);
            consumed += self.fill_window(&input[consumed..]);
            let all_in = consumed == input.len();
            let flushing = flush != Flush::None && all_in;
            self.process(flushing);
            if self.pending_left() > 0 {
                continue;
            }
            if !all_in {
                // Every round takes input or turns lookahead into output; a
                // round that does neither would repeat forever. Fail closed.
                if before == (consumed, self.strstart, self.lookahead) {
                    return Err(Error::config(ErrorKind::InvalidState));
                }
                continue;
            }
            if flush == Flush::None {
                return Ok(Progress::new(consumed, produced, Status::NeedsInput));
            }
            self.write_flush(flush);
            self.flush_written = true;
        }
    }

    /// Compresses `input` and appends to `out` until the flush point is
    /// written (with `Flush::None`, until all input is taken). Returns
    /// `Done`, or `NeedsInput` for `Flush::None`.
    ///
    /// # Errors
    ///
    /// As [`compress`](Deflater::compress), plus
    /// [`ErrorKind::AllocationFailed`] when `out` cannot grow.
    ///
    /// # Examples
    ///
    /// ```
    /// use compress_lib::deflate::{self, Deflater, Flush};
    ///
    /// let mut d = Deflater::default();
    /// let mut out = Vec::new();
    /// d.compress_vec(b"part one, ", &mut out, Flush::None)?;
    /// d.compress_vec(b"part two", &mut out, Flush::Finish)?;
    /// assert_eq!(deflate::decompress(&out)?, b"part one, part two");
    /// # Ok::<(), compress_lib::Error>(())
    /// ```
    pub fn compress_vec(
        &mut self,
        input: &[u8],
        out: &mut Vec<u8>,
        flush: Flush,
    ) -> Result<Progress, Error> {
        let start = out.len();
        let mut consumed = 0;
        loop {
            let have = out.len();
            let room = (input.len() - consumed) / 2 + 1024;
            if out.capacity() - have < room && out.try_reserve(room).is_err() {
                return Err(Error::config(ErrorKind::AllocationFailed));
            }
            let spare = out.capacity() - have;
            out.resize(have + spare, 0);
            let p = self.compress(&input[consumed..], &mut out[have..], flush);
            match p {
                Ok(p) => {
                    out.truncate(have + p.produced);
                    consumed += p.consumed;
                    if p.status != Status::NeedsOutput {
                        return Ok(Progress::new(consumed, out.len() - start, p.status));
                    }
                }
                Err(e) => {
                    out.truncate(have);
                    return Err(e);
                }
            }
        }
    }

    fn pending_left(&self) -> usize {
        self.pending.len() - self.pending_pos
    }

    /// Moves pending output into `output[*produced..]`.
    fn drain(&mut self, output: &mut [u8], produced: &mut usize) {
        let n = self.pending_left().min(output.len() - *produced);
        output[*produced..*produced + n]
            .copy_from_slice(&self.pending[self.pending_pos..self.pending_pos + n]);
        self.pending_pos += n;
        *produced += n;
        self.total_out += n as u64;
        if self.pending_pos == self.pending.len() {
            self.pending.clear();
            self.pending_pos = 0;
        }
    }

    /// Copies input into the window, sliding it first when the lookahead
    /// would not fit. Returns the bytes taken.
    fn fill_window(&mut self, input: &[u8]) -> usize {
        let size = self.win.len();
        if size - self.strstart - self.lookahead < MIN_LOOKAHEAD
            && self.strstart >= self.keep + self.wsize
        {
            self.slide();
        }
        let room = size - self.strstart - self.lookahead;
        let n = room.min(input.len());
        let at = self.strstart + self.lookahead;
        self.win[at..at + n].copy_from_slice(&input[..n]);
        self.lookahead += n;
        self.total_in += n as u64;
        n
    }

    /// Slides the window down by a whole number of windows, keeping at
    /// least one window of history before `strstart`.
    fn slide(&mut self) {
        let shift = (self.strstart - self.keep) / self.wsize * self.wsize;
        if shift == 0 {
            return;
        }
        if self.level == 0 && self.block_start < shift as isize {
            // Stored blocks are cut from the window; level 0 writes them
            // before the data leaves.
            self.end_block(false);
        }
        let end = self.strstart + self.lookahead;
        self.win.copy_within(shift..end, 0);
        self.strstart -= shift;
        self.match_start = self.match_start.saturating_sub(shift);
        self.prev_match = self.prev_match.saturating_sub(shift);
        self.block_start -= shift as isize;
        // Positions are below 2^16 and so is the shift; entries that slide
        // out become 0, "none".
        let s = shift as u32;
        for h in self.head.iter_mut().chain(self.prev.iter_mut()) {
            *h = u32::from(*h).saturating_sub(s) as u16;
        }
    }

    #[inline]
    fn hash(&self, pos: usize) -> usize {
        let v = u32::from(self.win[pos])
            | (u32::from(self.win[pos + 1]) << 8)
            | (u32::from(self.win[pos + 2]) << 16);
        (v.wrapping_mul(0x9E37_79B1) >> (32 - self.hash_bits)) as usize
    }

    /// Inserts `pos` into the hash chains; returns the previous head (0 for
    /// none).
    #[inline]
    fn insert(&mut self, pos: usize) -> usize {
        let h = self.hash(pos);
        let head = self.head[h];
        self.prev[pos & (self.wsize - 1)] = head;
        self.head[h] = pos as u16;
        head as usize
    }

    /// The longest match at `strstart` along the chain from `cur`, better
    /// than `prev_length`; sets `match_start`.
    fn longest_match(&mut self, mut cur: usize) -> usize {
        let p = self.params;
        let mut chain = p.chain;
        if self.prev_length >= p.good {
            chain >>= 2;
        }
        let max = MAX_MATCH.min(self.lookahead);
        let nice = p.nice.min(max);
        let keep = self.keep;
        let mut best = self.prev_length;
        let win = &self.win[..];
        let s = self.strstart;
        loop {
            if cur == 0 || cur >= s || s - cur > keep || best >= max {
                break;
            }
            if best < max
                && win[cur + best] == win[s + best]
                && win[cur] == win[s]
                && win[cur + 1] == win[s + 1]
            {
                let len = common_prefix(win, cur, s, max);
                if len > best {
                    self.match_start = cur;
                    best = len;
                    if len >= nice {
                        break;
                    }
                }
            }
            chain -= 1;
            if chain == 0 {
                break;
            }
            let next = self.prev[cur & (self.wsize - 1)] as usize;
            if next >= cur {
                break;
            }
            cur = next;
        }
        best.min(self.lookahead)
    }

    /// Records a literal. The caller ends the block when the symbol buffer
    /// is full, once `strstart` covers the symbol's input.
    fn literal(&mut self, b: u8) {
        self.syms.push(u32::from(b));
        self.scratch.lit_freq[b as usize] += 1;
    }

    /// Records a match (see [`literal`](Self::literal)).
    fn matched(&mut self, dist: usize, len: usize) {
        self.syms
            .push(((dist as u32) << 16) | (len - MIN_MATCH) as u32);
        self.scratch.lit_freq[257 + LENGTH_CODE[len - MIN_MATCH] as usize] += 1;
        self.scratch.dist_freq[dist_code(dist)] += 1;
    }

    /// Ends the block when the symbol buffer is full.
    #[inline]
    fn block_full(&mut self) {
        if self.syms.len() >= self.sym_cap {
            self.end_block(false);
        }
    }

    /// Turns the lookahead into symbols. Without `flushing`, stops while
    /// fewer than `MIN_LOOKAHEAD` bytes wait (more input could extend a
    /// match); with it, uses everything.
    fn process(&mut self, flushing: bool) {
        if self.level == 0 {
            let chunk = STORED_CHUNK.min(self.win.len() / 2);
            // A window too full to take more input and not yet able to slide
            // must be written out too, or the stream would stall.
            let full = self.strstart + self.lookahead + MIN_LOOKAHEAD > self.win.len();
            while self.lookahead >= chunk || ((flushing || full) && self.lookahead > 0) {
                let n = self.lookahead.min(65_535);
                self.strstart += n;
                self.lookahead -= n;
                self.end_block(false);
            }
            return;
        }
        if self.params.lazy_matching {
            self.process_lazy(flushing);
        } else {
            self.process_greedy(flushing);
        }
    }

    fn process_greedy(&mut self, flushing: bool) {
        loop {
            if self.lookahead < MIN_LOOKAHEAD && (!flushing || self.lookahead == 0) {
                return;
            }
            let mut head = 0;
            if self.lookahead >= MIN_MATCH {
                head = self.insert(self.strstart);
            }
            self.match_length = MIN_MATCH - 1;
            if head != 0 && self.strstart - head <= self.keep {
                self.prev_length = MIN_MATCH - 1;
                self.match_length = self.longest_match(head);
            }
            if self.match_length >= MIN_MATCH {
                let len = self.match_length;
                self.matched(self.strstart - self.match_start, len);
                self.lookahead -= len;
                if len <= self.params.lazy && self.lookahead >= MIN_MATCH {
                    for _ in 1..len {
                        self.strstart += 1;
                        self.insert(self.strstart);
                    }
                    self.strstart += 1;
                } else {
                    self.strstart += len;
                }
            } else {
                self.literal(self.win[self.strstart]);
                self.lookahead -= 1;
                self.strstart += 1;
            }
            self.block_full();
        }
    }

    fn process_lazy(&mut self, flushing: bool) {
        loop {
            if self.lookahead < MIN_LOOKAHEAD && (!flushing || self.lookahead == 0) {
                break;
            }
            let mut head = 0;
            if self.lookahead >= MIN_MATCH {
                head = self.insert(self.strstart);
            }
            self.prev_length = self.match_length;
            self.prev_match = self.match_start;
            self.match_length = MIN_MATCH - 1;
            if head != 0 && self.prev_length < self.params.lazy && self.strstart - head <= self.keep
            {
                self.match_length = self.longest_match(head);
                if self.match_length == MIN_MATCH && self.strstart - self.match_start > TOO_FAR {
                    self.match_length = MIN_MATCH - 1;
                }
            }
            if self.prev_length >= MIN_MATCH && self.match_length <= self.prev_length {
                let max_insert = self.strstart + self.lookahead - MIN_MATCH;
                let len = self.prev_length;
                self.matched(self.strstart - 1 - self.prev_match, len);
                self.lookahead -= len - 1;
                for _ in 0..len - 2 {
                    self.strstart += 1;
                    if self.strstart <= max_insert {
                        self.insert(self.strstart);
                    }
                }
                self.match_available = false;
                self.match_length = MIN_MATCH - 1;
                self.strstart += 1;
                self.block_full();
            } else if self.match_available {
                // The literal is the byte before `strstart`: end the block
                // before moving on, so the block's input covers it exactly.
                self.literal(self.win[self.strstart - 1]);
                self.block_full();
                self.strstart += 1;
                self.lookahead -= 1;
            } else {
                self.match_available = true;
                self.strstart += 1;
                self.lookahead -= 1;
            }
        }
        if flushing && self.match_available {
            self.literal(self.win[self.strstart - 1]);
            self.match_available = false;
            self.block_full();
        }
    }

    // ---- bit output ----------------------------------------------------

    #[inline]
    fn put(&mut self, value: u32, bits: u32) {
        self.bitbuf |= u64::from(value) << self.bitcnt;
        self.bitcnt += bits;
        if self.bitcnt >= 32 {
            self.pending
                .extend_from_slice(&(self.bitbuf as u32).to_le_bytes());
            self.bitbuf >>= 32;
            self.bitcnt -= 32;
        }
    }

    /// Writes out whole bytes and pads the last one with zero bits.
    fn align(&mut self) {
        while self.bitcnt > 0 {
            self.pending.push(self.bitbuf as u8);
            self.bitbuf >>= 8;
            self.bitcnt = self.bitcnt.saturating_sub(8);
        }
        self.bitbuf = 0;
    }

    fn write_flush(&mut self, flush: Flush) {
        match flush {
            Flush::None => {}
            Flush::Finish => {
                self.end_block(true);
                self.align();
            }
            Flush::Sync | Flush::Full => {
                self.end_block(false);
                // An empty stored block: header, pad, 00 00 FF FF.
                self.put(0, 3);
                self.align();
                self.pending.extend_from_slice(&[0, 0, 0xFF, 0xFF]);
                if flush == Flush::Full {
                    self.head.fill(0);
                    self.match_length = MIN_MATCH - 1;
                    self.prev_length = MIN_MATCH - 1;
                }
            }
        }
    }

    /// Writes the current block (if it has data, or if it is the last) as
    /// the cheapest of stored, fixed, and dynamic.
    fn end_block(&mut self, last: bool) {
        let stored_len = (self.strstart as isize - self.block_start).max(0) as usize;
        if self.syms.is_empty() && stored_len == 0 && !last {
            return;
        }
        let stored_ok = self.block_start >= 0;
        if self.level == 0 && stored_ok {
            self.write_stored(self.block_start as usize, stored_len, last);
        } else {
            self.write_huffman_or_stored(stored_ok, stored_len, last);
        }
        self.syms.clear();
        self.scratch.lit_freq.fill(0);
        self.scratch.dist_freq.fill(0);
        self.block_start = self.strstart as isize;
    }

    fn write_stored(&mut self, start: usize, len: usize, last: bool) {
        let mut at = start;
        let mut left = len;
        loop {
            let n = left.min(65_535);
            let final_piece = n == left;
            self.put(u32::from(last && final_piece), 1);
            self.put(0, 2);
            self.align();
            self.pending.extend_from_slice(&(n as u16).to_le_bytes());
            self.pending.extend_from_slice(&(!(n as u16)).to_le_bytes());
            self.pending.extend_from_slice(&self.win[at..at + n]);
            at += n;
            left -= n;
            if final_piece {
                break;
            }
        }
    }

    fn write_huffman_or_stored(&mut self, stored_ok: bool, stored_len: usize, last: bool) {
        let s = &mut *self.scratch;
        s.lit_freq[256] = 1;
        // Every block gets at least two literal/length codes and two
        // distance codes, as zlib does, so no decoder meets the one-code
        // special case.
        let lit_used = s.lit_freq.iter().filter(|&&f| f != 0).count();
        if lit_used < 2 {
            s.lit_freq[if s.lit_freq[0] == 0 { 0 } else { 1 }] += 1;
        }
        let dist_used = s.dist_freq.iter().filter(|&&f| f != 0).count();
        if dist_used == 0 {
            s.dist_freq[0] = 1;
            s.dist_freq[1] = 1;
        } else if dist_used == 1 {
            let d = if s.dist_freq[0] == 0 { 0 } else { 1 };
            s.dist_freq[d] = 1;
        }
        huffman::lengths(&s.lit_freq, 15, &mut s.lit_lens);
        huffman::lengths(&s.dist_freq, 15, &mut s.dist_lens);

        // Payload bits under each code; extra bits are the same for both.
        let mut extra: u64 = 0;
        let mut dyn_bits: u64 = 0;
        let mut fix_bits: u64 = 0;
        for sym in 0..286 {
            let f = u64::from(s.lit_freq[sym]);
            if f == 0 {
                continue;
            }
            dyn_bits += f * u64::from(s.lit_lens[sym]);
            fix_bits += f * u64::from(FIXED_LIT_LENS[sym]);
            if sym >= 257 {
                extra += f * u64::from(LENGTH_EXTRA[sym - 257]);
            }
        }
        for ((&f, &len), &x) in s.dist_freq.iter().zip(&s.dist_lens).zip(&DIST_EXTRA) {
            let f = u64::from(f);
            dyn_bits += f * u64::from(len);
            fix_bits += f * 5;
            extra += f * u64::from(x);
        }
        // The forced dummy symbols are counted above but never written: the
        // estimate is a few bits high for both codes alike.
        let hlit = 257
            + s.lit_lens[257..286]
                .iter()
                .rposition(|&l| l != 0)
                .map_or(0, |i| i + 1);
        let hdist = 1 + s.dist_lens.iter().rposition(|&l| l != 0).unwrap_or(0);
        rle_lengths(s, hlit, hdist);
        huffman::lengths(&s.pre_freq, 7, &mut s.pre_lens);
        let hclen = 4 + PRECODE_ORDER
            .iter()
            .rposition(|&i| s.pre_lens[i] != 0)
            .map_or(0, |i| (i + 1).saturating_sub(4));
        let mut header_bits = 14 + 3 * hclen as u64;
        for &(sym, _) in &s.rle[..s.rle_len] {
            header_bits += u64::from(s.pre_lens[sym as usize]);
            header_bits += match sym {
                16 => 2,
                17 => 3,
                18 => 7,
                _ => 0,
            };
        }
        let dynamic = 3 + header_bits + dyn_bits + extra;
        let fixed = 3 + fix_bits + extra;
        let pad = u64::from((8 - (self.bitcnt + 3) % 8) % 8);
        let pieces = (stored_len as u64).div_ceil(65_535).max(1);
        let stored = pieces * (3 + 32) + pad + 8 * stored_len as u64 + (pieces - 1) * 5;

        if stored_ok && stored <= fixed.min(dynamic) {
            self.write_stored(self.block_start as usize, stored_len, last);
            return;
        }
        if fixed <= dynamic {
            self.put(u32::from(last), 1);
            self.put(1, 2);
            let mut lit_codes = [0u16; 288];
            huffman::codes(&FIXED_LIT_LENS, &mut lit_codes);
            let mut dist_codes = [0u16; 30];
            huffman::codes(&[5u8; 30], &mut dist_codes);
            self.write_symbols(&lit_codes, &FIXED_LIT_LENS, &dist_codes, &[5u8; 30]);
        } else {
            self.put(u32::from(last), 1);
            self.put(2, 2);
            self.put((hlit - 257) as u32, 5);
            self.put((hdist - 1) as u32, 5);
            self.put((hclen - 4) as u32, 4);
            let s = &mut *self.scratch;
            huffman::codes(&s.pre_lens, &mut s.pre_codes);
            huffman::codes(&s.lit_lens, &mut s.lit_codes[..286]);
            huffman::codes(&s.dist_lens, &mut s.dist_codes);
            for &i in &PRECODE_ORDER[..hclen] {
                let l = self.scratch.pre_lens[i];
                self.put(u32::from(l), 3);
            }
            for k in 0..self.scratch.rle_len {
                let (sym, value) = self.scratch.rle[k];
                let code = self.scratch.pre_codes[sym as usize];
                let len = self.scratch.pre_lens[sym as usize];
                self.put(u32::from(code), u32::from(len));
                match sym {
                    16 => self.put(u32::from(value), 2),
                    17 => self.put(u32::from(value), 3),
                    18 => self.put(u32::from(value), 7),
                    _ => {}
                }
            }
            let lit_codes = self.scratch.lit_codes;
            let lit_lens = self.scratch.lit_lens;
            let dist_codes = self.scratch.dist_codes;
            let dist_lens = self.scratch.dist_lens;
            self.write_symbols(&lit_codes, &lit_lens, &dist_codes, &dist_lens);
        }
    }

    fn write_symbols(
        &mut self,
        lit_codes: &[u16],
        lit_lens: &[u8],
        dist_codes: &[u16],
        dist_lens: &[u8],
    ) {
        let syms = core::mem::take(&mut self.syms);
        for &sym in &syms {
            let dist = (sym >> 16) as usize;
            let litlen = (sym & 0xFFFF) as usize;
            if dist == 0 {
                self.put(u32::from(lit_codes[litlen]), u32::from(lit_lens[litlen]));
            } else {
                let code = LENGTH_CODE[litlen] as usize;
                self.put(
                    u32::from(lit_codes[257 + code]),
                    u32::from(lit_lens[257 + code]),
                );
                let extra = u32::from(LENGTH_EXTRA[code]);
                if extra != 0 {
                    self.put(
                        (litlen + MIN_MATCH - LENGTH_BASE[code] as usize) as u32,
                        extra,
                    );
                }
                let d = dist_code(dist);
                self.put(u32::from(dist_codes[d]), u32::from(dist_lens[d]));
                let extra = u32::from(DIST_EXTRA[d]);
                if extra != 0 {
                    self.put((dist - DIST_BASE[d] as usize) as u32, extra);
                }
            }
        }
        self.syms = syms;
        self.put(u32::from(lit_codes[256]), u32::from(lit_lens[256]));
    }
}

/// Bytes in common at `a` and `b` (`a < b`), at most `max`.
#[inline]
fn common_prefix(win: &[u8], a: usize, b: usize, max: usize) -> usize {
    let mut n = 0;
    while n + 8 <= max {
        let mut x = [0u8; 8];
        let mut y = [0u8; 8];
        x.copy_from_slice(&win[a + n..a + n + 8]);
        y.copy_from_slice(&win[b + n..b + n + 8]);
        let diff = u64::from_le_bytes(x) ^ u64::from_le_bytes(y);
        if diff != 0 {
            return n + (diff.trailing_zeros() / 8) as usize;
        }
        n += 8;
    }
    while n < max && win[a + n] == win[b + n] {
        n += 1;
    }
    n
}

/// Run-length codes the literal/length lengths `[..hlit]` followed by the
/// distance lengths `[..hdist]` with symbols 16 (repeat previous 3 to 6), 17
/// (zeros 3 to 10), and 18 (zeros 11 to 138), and counts precode
/// frequencies.
fn rle_lengths(s: &mut BlockScratch, hlit: usize, hdist: usize) {
    let mut all = [0u8; 316];
    all[..hlit].copy_from_slice(&s.lit_lens[..hlit]);
    all[hlit..hlit + hdist].copy_from_slice(&s.dist_lens[..hdist]);
    let all = &all[..hlit + hdist];
    s.pre_freq = [0; 19];
    s.rle_len = 0;
    let push = |s: &mut BlockScratch, sym: u8, value: u8| {
        s.rle[s.rle_len] = (sym, value);
        s.rle_len += 1;
        s.pre_freq[sym as usize] += 1;
    };
    let mut i = 0;
    while i < all.len() {
        let v = all[i];
        let mut run = 1;
        while i + run < all.len() && all[i + run] == v {
            run += 1;
        }
        i += run;
        if v == 0 {
            while run >= 11 {
                let n = run.min(138);
                push(s, 18, (n - 11) as u8);
                run -= n;
            }
            if run >= 3 {
                push(s, 17, (run - 3) as u8);
                run = 0;
            }
            for _ in 0..run {
                push(s, 0, 0);
            }
        } else {
            push(s, v, 0);
            run -= 1;
            while run >= 3 {
                let n = run.min(6);
                push(s, 16, (n - 3) as u8);
                run -= n;
            }
            for _ in 0..run {
                push(s, v, 0);
            }
        }
    }
}
