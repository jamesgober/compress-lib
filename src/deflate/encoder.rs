//! The raw DEFLATE (RFC 1951) encoder.
//!
//! Input is copied into a sliding window; a hash of three bytes indexes
//! chains of earlier positions (level 1 keeps only the newest position per
//! hash); greedy (levels 1 to 3) or lazy (4 to 9) matching turns the window
//! into literal and match symbols; level 0 cuts stored blocks of a fixed
//! size. Every 16,384 symbols (fewer for small windows) a block is written
//! as stored, fixed, or dynamic Huffman, whichever is smallest. Output waits
//! in a pending buffer of at most one block until the caller drains it.

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
/// Level 1: a match of length 3 further back than this is written as
/// literals.
const FAR3: usize = 8192;
/// Bytes past the window's logical end, so four-byte loads at any position
/// of the window are in bounds.
const WIN_PAD: usize = 8;
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
        1 => (4, 4, 8, 1, false),
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
///
/// # Examples
///
/// ```
/// use compress_lib::deflate::{self, Deflater, Flush, Inflater, Status};
///
/// let mut d = Deflater::default();
/// let mut wire = Vec::new();
/// d.compress_vec(b"first ", &mut wire, Flush::Sync)?;   // decodable up to here
/// let mut i = Inflater::new();
/// let mut seen = Vec::new();
/// assert_eq!(i.decompress_vec(&wire, &mut seen)?.status, Status::NeedsInput);
/// assert_eq!(seen, b"first ");
/// d.compress_vec(b"second", &mut wire, Flush::Finish)?;  // the end of the stream
/// assert_eq!(deflate::decompress(&wire)?, b"first second");
/// # Ok::<(), compress_lib::Error>(())
/// ```
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

/// Memory settings for a [`Deflater`]: the hash table and the symbol
/// buffer, the two largest parts after the window.
///
/// | Setting | Default | Memory | Effect |
/// |---|---|---|---|
/// | hash bits | window bits + 1, 9 to 15 (12 at most at level 1) | 2 bytes per entry | fewer bits: less memory, more collisions, slightly larger output |
/// | block symbols | 2^(window bits + 1), 1,024 to 16,384 | 4 bytes per symbol, plus up to 4 bytes of pending output | fewer: less memory, more block headers |
///
/// # Examples
///
/// ```
/// use compress_lib::deflate::MemoryConfig;
///
/// let config = MemoryConfig::new().with_hash_bits(11).with_block_symbols(4096);
/// assert_eq!((config.hash_bits(), config.block_symbols()), (11, 4096));
/// ```
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub struct MemoryConfig {
    /// 0: chosen from the window bits.
    hash_bits: u8,
    /// 0: chosen from the window bits.
    block_symbols: usize,
}

impl MemoryConfig {
    /// Both sizes chosen from the window bits.
    ///
    /// # Examples
    ///
    /// ```
    /// assert_eq!(compress_lib::deflate::MemoryConfig::new().hash_bits(), 0);
    /// ```
    #[must_use]
    pub const fn new() -> Self {
        Self {
            hash_bits: 0,
            block_symbols: 0,
        }
    }

    /// Sets the hash table to `2^bits` entries; clamped to 8 to 16 (0 keeps
    /// the default).
    ///
    /// # Examples
    ///
    /// ```
    /// assert_eq!(compress_lib::deflate::MemoryConfig::new().with_hash_bits(20).hash_bits(), 16);
    /// ```
    #[must_use]
    pub const fn with_hash_bits(mut self, bits: u8) -> Self {
        self.hash_bits = if bits == 0 {
            0
        } else if bits < 8 {
            8
        } else if bits > 16 {
            16
        } else {
            bits
        };
        self
    }

    /// Sets the symbols per block; clamped to 256 to 65,536 (0 keeps the
    /// default).
    ///
    /// # Examples
    ///
    /// ```
    /// assert_eq!(compress_lib::deflate::MemoryConfig::new().with_block_symbols(100).block_symbols(), 256);
    /// ```
    #[must_use]
    pub const fn with_block_symbols(mut self, symbols: usize) -> Self {
        self.block_symbols = if symbols == 0 {
            0
        } else if symbols < 256 {
            256
        } else if symbols > 65_536 {
            65_536
        } else {
            symbols
        };
        self
    }

    /// The hash bits set (0: chosen from the window bits).
    ///
    /// # Examples
    ///
    /// ```
    /// assert_eq!(compress_lib::deflate::MemoryConfig::new().with_hash_bits(10).hash_bits(), 10);
    /// ```
    #[must_use]
    pub const fn hash_bits(&self) -> u8 {
        self.hash_bits
    }

    /// The symbols per block set (0: chosen from the window bits).
    ///
    /// # Examples
    ///
    /// ```
    /// assert_eq!(compress_lib::deflate::MemoryConfig::new().block_symbols(), 0);
    /// ```
    #[must_use]
    pub const fn block_symbols(&self) -> usize {
        self.block_symbols
    }
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

impl BlockScratch {
    fn new() -> Self {
        Self {
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
        }
    }
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
    memory: MemoryConfig,
    level: u8,
    params: Params,
    window_bits: u8,
    /// Window buffer of `wsize * span` bytes; positions are indices into it.
    win: Vec<u8>,
    /// Logical size of `win`; the vector has `WIN_PAD` more zero bytes so
    /// four-byte loads near the end stay in bounds.
    size: usize,
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
    /// Bytes hashed per position: 3 at levels 1 to 3, 4 at the lazy levels
    /// 4 to 9 (D-065), as the mask over a four-byte load.
    hash_mask: u32,
    /// Chain steps past `CHAIN_FREE` the lazy search may still take:
    /// `CHAIN_BUDGET` more per input byte, at most `CHAIN_BANK` banked
    /// (T-14, D-065). Earned up to the stream position `budget_pos`.
    chain_budget: i64,
    budget_pos: u64,
    /// Bytes the window has slid since the stream began, so a window
    /// position plus this is a position in the stream.
    slid: u64,
    head: Vec<u16>,
    prev: Vec<u16>,
    match_available: bool,
    match_length: usize,
    match_start: usize,
    prev_length: usize,
    prev_match: usize,
    /// Level 1: positions in a row without a match. Kept across calls so the
    /// output does not depend on how the input was split.
    misses: usize,
    /// Symbols of the current block, `syms[..nsyms]`: `dist << 16 | litlen`,
    /// where a literal has `dist == 0` and a match stores `length - 3`. The
    /// vector is `sym_cap` long, so symbols are stored by index.
    syms: Vec<u32>,
    nsyms: usize,
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
        Self::build(level.get(), 15, MemoryConfig::new())
    }

    fn build(level: u8, window_bits: u8, memory: MemoryConfig) -> Self {
        let wsize = 1usize << window_bits;
        // At least 8 KiB of buffer, so small windows still read input in
        // useful pieces; always a whole number of windows (the hash chains
        // are indexed modulo the window).
        let span = (8192 / wsize).max(2);
        // Level 1 probes once, so a table that stays in the first-level cache
        // is faster and loses little (D-040).
        let max_hash = if level == 1 { 12 } else { 15 };
        let hash_bits = match memory.hash_bits {
            0 => u32::from(window_bits + 1).clamp(9, max_hash),
            bits => u32::from(bits),
        };
        let sym_cap = match memory.block_symbols {
            0 => (1usize << (window_bits + 1)).clamp(1024, 16_384),
            n => n,
        };
        Self {
            memory,
            level,
            params: params(level),
            window_bits,
            win: vec![0; wsize * span + WIN_PAD],
            size: wsize * span,
            wsize,
            keep: if span >= 3 {
                wsize
            } else {
                wsize - MIN_LOOKAHEAD
            },
            // Position 0 means "none" in the hash chains, so data starts at 1.
            strstart: 1,
            lookahead: 0,
            block_start: 1,
            hash_bits,
            hash_mask: hash_mask(level),
            chain_budget: CHAIN_BANK,
            budget_pos: 0,
            slid: 0,
            head: vec![0; 1 << hash_bits],
            prev: vec![0; wsize],
            match_available: false,
            match_length: MIN_MATCH - 1,
            match_start: 0,
            prev_length: MIN_MATCH - 1,
            prev_match: 0,
            misses: 0,
            syms: vec![0; sym_cap],
            nsyms: 0,
            sym_cap,
            scratch: alloc::boxed::Box::new(BlockScratch::new()),
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
        Ok(Self::build(self.level, bits, self.memory))
    }

    /// Sizes the hash table and the symbol buffer explicitly (see
    /// [`MemoryConfig`]), for many connections on little memory or for a
    /// little more ratio.
    ///
    /// # Examples
    ///
    /// ```
    /// use compress_lib::{Level, deflate::{self, Deflater, Flush, MemoryConfig}};
    ///
    /// let lean = MemoryConfig::new().with_hash_bits(9).with_block_symbols(512);
    /// let mut d = Deflater::new(Level::FAST).with_window_bits(9)?.with_memory(lean);
    /// assert!(d.memory_in_use() < Deflater::new(Level::FAST).with_window_bits(9)?.memory_in_use());
    /// let mut out = Vec::new();
    /// d.compress_vec(b"small and lean", &mut out, Flush::Finish)?;
    /// assert_eq!(deflate::decompress(&out)?, b"small and lean");
    /// # Ok::<(), compress_lib::Error>(())
    /// ```
    #[must_use]
    pub fn with_memory(self, memory: MemoryConfig) -> Self {
        Self::build(self.level, self.window_bits, memory)
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
    /// Memory stays at its built size when the change follows a call that
    /// returned `NeedsInput` or `Done`. Changing the level while output is
    /// still waiting (`NeedsOutput`) compresses the waiting input under the
    /// old level at once, and its blocks wait with that output, so the
    /// output buffer may grow past its built size.
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
            self.process(true, false);
            self.end_block(false);
        } else if self.params.lazy_matching && !next.lazy_matching && self.match_available {
            // Lazy matching holds the byte before `strstart` back; greedy
            // matching does not know about it.
            self.literal(self.win[self.strstart - 1]);
            self.match_available = false;
            self.block_full();
        }
        if self.params.chain == 1 && next.chain > 1 {
            // Level 1 left the chains stale (D-055); empty chains are only
            // shorter.
            self.prev.fill(0);
        }
        if hash_mask(level) != self.hash_mask {
            // The chains were built on the other hash width. Empty them (only
            // shorter), so every head entry has the width `reset` assumes.
            self.head.fill(0);
            self.prev.fill(0);
            self.hash_mask = hash_mask(level);
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
        // Clear only the hash heads this stream used when it was short and
        // never slid (a small WebSocket message touches a few dozen of 32,768
        // entries); otherwise clear them all.
        let end = self.strstart + self.lookahead;
        if self.total_in as usize + 1 < self.size && end < self.head.len() / 4 {
            let shift = 32 - self.hash_bits;
            let mask = self.hash_mask;
            for pos in 1..end.saturating_sub(MIN_MATCH - 1) {
                self.head[hash(load32(&self.win, pos), mask, shift)] = 0;
            }
        } else {
            self.head.fill(0);
        }
        self.strstart = 1;
        self.lookahead = 0;
        self.block_start = 1;
        self.chain_budget = CHAIN_BANK;
        self.budget_pos = 0;
        self.slid = 0;
        self.match_available = false;
        self.match_length = MIN_MATCH - 1;
        self.prev_length = MIN_MATCH - 1;
        self.misses = 0;
        self.nsyms = 0;
        self.pending.clear();
        self.pending_pos = 0;
        self.bitbuf = 0;
        self.bitcnt = 0;
        self.total_in = 0;
        self.total_out = 0;
        self.flush_written = false;
        self.finished = false;
    }

    /// Zeroes the window (the plaintext history), the hash chains, the
    /// symbol buffer, the pending output, and the block scratch, spare
    /// capacity included, then resets the deflater for a new stream. For
    /// connections that carried secrets: call it before the deflater is
    /// dropped or reused. The allocations are kept.
    ///
    /// The zeroing goes through [`core::hint::black_box`] so it is not
    /// removed as a dead store; without `unsafe` volatile writes that is a
    /// strong hint to the compiler, not a guarantee.
    ///
    /// # Examples
    ///
    /// ```
    /// use compress_lib::deflate::{Deflater, Flush};
    ///
    /// let mut d = Deflater::default();
    /// d.compress_vec(b"password=hunter2", &mut Vec::new(), Flush::Sync)?;
    /// d.wipe();
    /// assert_eq!(d.total_in(), 0);
    /// # Ok::<(), compress_lib::Error>(())
    /// ```
    pub fn wipe(&mut self) {
        crate::wipe_vec(&mut self.win);
        crate::wipe_vec(&mut self.head);
        crate::wipe_vec(&mut self.prev);
        crate::wipe_vec(&mut self.syms);
        crate::wipe_vec(&mut self.pending);
        *self.scratch = BlockScratch::new();
        core::hint::black_box(&*self.scratch);
        self.bitbuf = 0;
        self.reset();
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
        if self.total_in != 0 || self.lookahead != 0 || self.strstart != 1 {
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
            while pos + hash_len(self.hash_mask) <= end {
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
            self.process(flushing, true);
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
            // Zero-fill only this step's room, not all spare capacity: the
            // spare grows with the output, so filling it on every call
            // would make many small calls quadratic (T-5).
            out.resize(have + room, 0);
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
        let size = self.size;
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
        self.slid += shift as u64;
        // Positions are below 2^16 and so is the shift; entries that slide
        // out become 0, "none".
        let s = shift as u32;
        for h in self.head.iter_mut().chain(self.prev.iter_mut()) {
            *h = u32::from(*h).saturating_sub(s) as u16;
        }
    }

    /// Inserts `pos` into the hash chains; returns the previous head (0 for
    /// none).
    #[inline]
    fn insert(&mut self, pos: usize) -> usize {
        insert_at(
            &mut self.head,
            &mut self.prev,
            &self.win,
            pos,
            self.hash_mask,
            32 - self.hash_bits,
            self.wsize - 1,
        )
    }

    /// Records a literal (set_level, flushes).
    fn literal(&mut self, b: u8) {
        self.syms[self.nsyms] = u32::from(b);
        self.nsyms += 1;
        self.scratch.lit_freq[b as usize] += 1;
    }

    /// Ends the block when the symbol buffer is full. With a held-back
    /// literal (lazy matching), the block stops before that byte.
    #[inline]
    fn block_full(&mut self) {
        if self.nsyms >= self.sym_cap {
            let held = usize::from(self.match_available);
            self.strstart -= held;
            self.end_block(false);
            self.strstart += held;
        }
    }

    /// Turns the lookahead into symbols. Without `flushing`, stops while
    /// fewer than `MIN_LOOKAHEAD` bytes wait (more input could extend a
    /// match); with it, uses everything.
    /// With `one_block`, returns once `pending` has no room for another
    /// block, so the caller drains it before the next (`pending` never grows,
    /// D-064).
    fn process(&mut self, flushing: bool, one_block: bool) {
        if self.level == 0 {
            // Blocks of exactly `chunk` bytes, so the cuts do not depend on
            // how the input arrived (D-057). A shorter block is cut only at a
            // flush, or when the window is completely full and cannot slide
            // yet: the same point however the input was split.
            let chunk = STORED_CHUNK.min(self.size / 2);
            while self.lookahead >= chunk {
                self.strstart += chunk;
                self.lookahead -= chunk;
                self.end_block(false);
            }
            let full = self.strstart + self.lookahead == self.size
                && self.strstart < self.keep + self.wsize;
            if (flushing || full) && self.lookahead > 0 {
                self.strstart += self.lookahead;
                self.lookahead = 0;
                self.end_block(false);
            }
            return;
        }
        loop {
            let used_up = if self.params.lazy_matching {
                self.lazy_run(flushing)
            } else {
                self.greedy_run(flushing)
            };
            self.block_full();
            if used_up {
                break;
            }
            // When the room left in `pending` could not take another block
            // of the largest size (4 bytes per symbol, the fixed-code bound
            // every block written is at most, plus the writer's slack), let
            // the caller drain it first, so `pending` never outgrows its
            // first allocation. Where blocks end does not change.
            if one_block && false {
                return;
            }
        }
        if flushing && self.match_available {
            self.literal(self.win[self.strstart - 1]);
            self.match_available = false;
            self.block_full();
        }
    }

    /// Level 1 while a whole match always fits in the lookahead: one probe
    /// per position and no chain, so no lookahead or chain checks inside the
    /// loop (D-055). Makes the same choices as [`greedy_run`](Self::greedy_run),
    /// which finishes what is left.
    fn fast_run(&mut self) {
        let lazy = self.params.lazy;
        let keep = self.keep;
        let shift = 32 - self.hash_bits;
        let cap = self.sym_cap;
        let mut s = self.strstart;
        let mut look = self.lookahead;
        let mut n = self.nsyms;
        let mut misses = self.misses;
        let win = &self.win[..];
        let head = &mut self.head[..];
        let syms = &mut self.syms[..];
        let freq = &mut *self.scratch;
        while look >= MIN_LOOKAHEAD && n < cap {
            let s4 = load32(win, s);
            let h = hash3(s4, shift);
            let cur = usize::from(head[h]);
            head[h] = s as u16;
            let mut len = 0;
            if cur != 0 && cur < s && s - cur <= keep && (load32(win, cur) ^ s4) & 0x00FF_FFFF == 0
            {
                len = MIN_MATCH
                    + common_prefix(win, cur + MIN_MATCH, s + MIN_MATCH, MAX_MATCH - MIN_MATCH);
                if len == MIN_MATCH && s - cur > FAR3 {
                    len = 0;
                }
            }
            if len >= MIN_MATCH {
                let dist = s - cur;
                syms[n] = ((dist as u32) << 16) | (len - MIN_MATCH) as u32;
                n += 1;
                freq.lit_freq[257 + LENGTH_CODE[len - MIN_MATCH] as usize] += 1;
                freq.dist_freq[dist_code(dist)] += 1;
                // At least MIN_LOOKAHEAD - MAX_MATCH bytes are left, enough to
                // hash every position inside a short match.
                look -= len;
                misses = 0;
                if len <= lazy {
                    for _ in 1..len {
                        s += 1;
                        head[hash3(load32(win, s), shift)] = s as u16;
                    }
                    s += 1;
                } else {
                    s += len;
                }
            } else {
                let b = win[s];
                syms[n] = u32::from(b);
                n += 1;
                freq.lit_freq[b as usize] += 1;
                look -= 1;
                s += 1;
                misses += 1;
                if misses > 64 {
                    // As in greedy_run (D-040).
                    let extra = ((misses - 64) >> 4).min(15).min(look).min(cap - n);
                    for &b in &win[s..s + extra] {
                        syms[n] = u32::from(b);
                        n += 1;
                        freq.lit_freq[b as usize] += 1;
                    }
                    s += extra;
                    look -= extra;
                }
            }
        }
        self.strstart = s;
        self.lookahead = look;
        self.nsyms = n;
        self.misses = misses;
    }

    /// Greedy matching (levels 1 to 3) with the hot state in locals. Runs
    /// until the lookahead is used up (`true`) or the symbol buffer is full
    /// (`false`).
    fn greedy_run(&mut self, flushing: bool) -> bool {
        if self.level == 1 {
            self.fast_run();
        }
        let p = self.params;
        let wmask = self.wsize - 1;
        let keep = self.keep;
        let shift = 32 - self.hash_bits;
        let cap = self.sym_cap;
        let mut s = self.strstart;
        let mut look = self.lookahead;
        let mut misses = self.misses;
        let skip = self.level == 1;
        // One probe per position never follows a chain, so level 1 keeps
        // only the heads (D-055).
        let chained = p.chain > 1;
        let win = &self.win[..];
        let head = &mut self.head[..];
        let prev = &mut self.prev[..];
        let syms = &mut self.syms[..];
        let mut n = self.nsyms;
        let freq = &mut *self.scratch;
        let used_up = loop {
            if look < MIN_LOOKAHEAD && (!flushing || look == 0) {
                break true;
            }
            if n >= cap {
                break false;
            }
            let mut len = 0;
            let mut start = 0;
            if look >= MIN_MATCH {
                let s4 = load32(win, s);
                let h = hash3(s4, shift);
                let mut cur = usize::from(head[h]);
                if chained {
                    prev[s & wmask] = head[h];
                }
                head[h] = s as u16;
                let max = MAX_MATCH.min(look);
                let mut chain = p.chain;
                if cur != 0 && cur < s && s - cur <= keep {
                    let low = s.saturating_sub(keep).max(1);
                    // As in lazy_run: the four bytes ending a longer match
                    // first, once there is a match to beat (D-058).
                    let mut end = 0;
                    let mut s_end = 0;
                    loop {
                        if (len < MIN_MATCH || load32(win, cur + end) == s_end)
                            && (load32(win, cur) ^ s4) & 0x00FF_FFFF == 0
                        {
                            let l = MIN_MATCH
                                + common_prefix(
                                    win,
                                    cur + MIN_MATCH,
                                    s + MIN_MATCH,
                                    max - MIN_MATCH,
                                );
                            if l > len {
                                len = l;
                                start = cur;
                                if l >= p.nice.min(max) {
                                    break;
                                }
                                end = len - MIN_MATCH;
                                s_end = load32(win, s + end);
                            }
                        }
                        chain -= 1;
                        if chain == 0 {
                            break;
                        }
                        let next = usize::from(prev[cur & wmask]);
                        if next >= cur || next < low {
                            break;
                        }
                        cur = next;
                    }
                }
            }
            if skip && len == MIN_MATCH && s - start > FAR3 {
                len = 0;
            }
            if len >= MIN_MATCH {
                let dist = s - start;
                syms[n] = ((dist as u32) << 16) | (len - MIN_MATCH) as u32;
                n += 1;
                freq.lit_freq[257 + LENGTH_CODE[len - MIN_MATCH] as usize] += 1;
                freq.dist_freq[dist_code(dist)] += 1;
                look -= len;
                misses = 0;
                if len <= p.lazy && look >= MIN_MATCH {
                    for _ in 1..len {
                        s += 1;
                        if chained {
                            insert_at(head, prev, win, s, HASH3, shift, wmask);
                        } else {
                            head[hash3(load32(win, s), shift)] = s as u16;
                        }
                    }
                    s += 1;
                } else {
                    s += len;
                }
            } else {
                let b = win[s];
                syms[n] = u32::from(b);
                n += 1;
                freq.lit_freq[b as usize] += 1;
                look -= 1;
                s += 1;
                misses += 1;
                // Level 1 on data that keeps missing (compressed or random
                // input): search only every few positions, more sparsely
                // the longer the run of misses (D-040).
                if skip && misses > 64 {
                    let extra = ((misses - 64) >> 4).min(15).min(look).min(cap - n);
                    for &b in &win[s..s + extra] {
                        syms[n] = u32::from(b);
                        n += 1;
                        freq.lit_freq[b as usize] += 1;
                    }
                    s += extra;
                    look -= extra;
                }
            }
        };
        self.strstart = s;
        self.lookahead = look;
        self.nsyms = n;
        self.misses = misses;
        self.match_length = MIN_MATCH - 1;
        used_up
    }

    /// Lazy matching (levels 4 to 9, zlib's deflate_slow) with the hot state
    /// in locals; returns as [`greedy_run`](Self::greedy_run).
    fn lazy_run(&mut self, flushing: bool) -> bool {
        let p = self.params;
        let wmask = self.wsize - 1;
        let keep = self.keep;
        let shift = 32 - self.hash_bits;
        // Lazy levels always hash four bytes (`hash_mask`), as constants
        // here so the loop keeps no register for them.
        debug_assert_eq!(self.hash_mask, HASH4);
        let mask = HASH4;
        // A position is hashed only with all its hashed bytes in the
        // lookahead, so no hash reads past the input (which would make the
        // chains depend on how the input arrived).
        let hlen = hash_len(HASH4);
        // Kept in memory and touched only by searches longer than
        // `CHAIN_FREE`, so it costs the short ones nothing.
        let mut budget = Budget {
            left: self.chain_budget,
            earned_to: self.budget_pos,
            base: self.slid,
            granted: false,
        };
        let cap = self.sym_cap;
        let mut s = self.strstart;
        let mut look = self.lookahead;
        let mut available = self.match_available;
        let mut match_length = self.match_length;
        let mut match_start = self.match_start;
        let mut prev_length;
        let mut prev_match;
        let win = &self.win[..];
        let head = &mut self.head[..];
        let prev = &mut self.prev[..];
        let syms = &mut self.syms[..];
        let mut n = self.nsyms;
        let freq = &mut *self.scratch;
        let used_up = loop {
            if look < MIN_LOOKAHEAD && (!flushing || look == 0) {
                break true;
            }
            if n >= cap {
                break false;
            }
            let mut cur = 0;
            let mut s4 = 0;
            if look >= hlen {
                s4 = load32(win, s);
                let h = hash(s4, mask, shift);
                cur = usize::from(head[h]);
                prev[s & wmask] = head[h];
                head[h] = s as u16;
            }
            prev_length = match_length;
            prev_match = match_start;
            match_length = MIN_MATCH - 1;
            if cur != 0 && prev_length < p.lazy && cur < s && s - cur <= keep {
                // Search for a match longer than the one held back.
                let max = MAX_MATCH.min(look);
                let nice = p.nice.min(max);
                let full = if prev_length >= p.good {
                    p.chain >> 2
                } else {
                    p.chain
                };
                // The first `CHAIN_FREE` steps are free; more come from the
                // budget, so input that keeps every chain full of short
                // matches gets about the search ordinary data uses, not ten
                // times more. Short walks never touch the budget.
                let mut chain = full.min(CHAIN_FREE);
                let mut extra = full - chain;
                let mut best = prev_length;
                // Candidates at or above `low` are inside the window.
                let low = s.saturating_sub(keep).max(1);
                // The four bytes ending a longer match, checked first (zlib's
                // scan end, two bytes wider): a candidate that differs there
                // cannot beat `best` (D-058).
                let mut end = best.max(MIN_MATCH) - MIN_MATCH;
                let mut s_end = load32(win, s + end);
                while best < max {
                    if (best < MIN_MATCH || load32(win, cur + end) == s_end)
                        && (load32(win, cur) ^ s4) & 0x00FF_FFFF == 0
                    {
                        let l = MIN_MATCH
                            + common_prefix(win, cur + MIN_MATCH, s + MIN_MATCH, max - MIN_MATCH);
                        if l > best {
                            best = l;
                            match_start = cur;
                            if l >= nice {
                                break;
                            }
                            end = best - MIN_MATCH;
                            s_end = load32(win, s + end);
                        }
                    }
                    chain -= 1;
                    if chain == 0 {
                        if extra == 0 {
                            break;
                        }
                        chain = budget.grant(s, extra);
                        if chain == 0 {
                            break;
                        }
                        extra -= chain;
                    }
                    let next = usize::from(prev[cur & wmask]);
                    if next >= cur || next < low {
                        break;
                    }
                    cur = next;
                }
                if budget.granted {
                    budget.refund(chain);
                }
                match_length = best.min(look);
                if match_length == MIN_MATCH && s - match_start > TOO_FAR {
                    match_length = MIN_MATCH - 1;
                }
            }
            if prev_length >= MIN_MATCH && match_length <= prev_length {
                let max_insert = (s + look).saturating_sub(hlen);
                let dist = s - 1 - prev_match;
                syms[n] = ((dist as u32) << 16) | (prev_length - MIN_MATCH) as u32;
                n += 1;
                freq.lit_freq[257 + LENGTH_CODE[prev_length - MIN_MATCH] as usize] += 1;
                freq.dist_freq[dist_code(dist)] += 1;
                look -= prev_length - 1;
                for _ in 0..prev_length - 2 {
                    s += 1;
                    if s <= max_insert {
                        insert_at(head, prev, win, s, mask, shift, wmask);
                    }
                }
                available = false;
                match_length = MIN_MATCH - 1;
                s += 1;
            } else if available {
                let b = win[s - 1];
                syms[n] = u32::from(b);
                n += 1;
                freq.lit_freq[b as usize] += 1;
                s += 1;
                look -= 1;
            } else {
                available = true;
                s += 1;
                look -= 1;
            }
        };
        self.strstart = s;
        self.lookahead = look;
        self.chain_budget = budget.left;
        self.budget_pos = budget.earned_to;
        self.nsyms = n;
        self.match_available = available;
        self.match_length = match_length;
        self.match_start = match_start;
        self.prev_length = match_length;
        used_up
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
        if self.nsyms == 0 && stored_len == 0 && !last {
            return;
        }
        let stored_ok = self.block_start >= 0;
        if self.level == 0 && stored_ok {
            self.write_stored(self.block_start as usize, stored_len, last);
        } else {
            self.write_huffman_or_stored(stored_ok, stored_len, last);
        }
        self.nsyms = 0;
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
            self.write_symbols(
                &lit_codes,
                &FIXED_LIT_LENS,
                &dist_codes,
                &[5u8; 30],
                fix_bits + extra,
            );
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
            self.write_symbols(
                &lit_codes,
                &lit_lens,
                &dist_codes,
                &dist_lens,
                dyn_bits + extra,
            );
        }
    }

    /// Writes the block's symbols and its end-of-block code; `payload_bits`
    /// is their size under these codes (counted when the block type was
    /// chosen, at least the true size).
    fn write_symbols(
        &mut self,
        lit_codes: &[u16],
        lit_lens: &[u8],
        dist_codes: &[u16],
        dist_lens: &[u8],
        payload_bits: u64,
    ) {
        // The bit buffer lives in locals for the whole block. A symbol is at
        // most 48 bits (a 15-bit length code with 5 extra bits, a 15-bit
        // distance code with 13), so with at most 7 bits left over it always
        // fits in 64: each symbol is one eight-byte store into room sized
        // for the whole block, then the whole bytes are kept (D-056). The
        // room is the block's counted size plus the eight bytes the last
        // store may reach past it, so `pending` never grows past the
        // capacity it was built with (`process` stops for a drain before
        // the room left could not take a block of 4 bytes per symbol,
        // D-064).
        let mut bits = self.bitbuf;
        let mut count = self.bitcnt;
        let out = &mut self.pending;
        let mut at = out.len();
        let room =
            usize::try_from((u64::from(count) + payload_bits).div_ceil(8)).unwrap_or(usize::MAX);
        out.resize(at + room + 8, 0);
        let buf = &mut out[..];
        let mut flush = |bits: &mut u64, count: &mut u32, at: &mut usize| {
            buf[*at..*at + 8].copy_from_slice(&bits.to_le_bytes());
            let whole = *count & !7;
            *at += (whole >> 3) as usize;
            // `whole` can be 64 only with count 64, which a symbol never
            // reaches; checked shifts keep it defined anyway.
            *bits = bits.checked_shr(whole).unwrap_or(0);
            *count -= whole;
        };
        // Per-block tables, so a literal is one load and a match two: the
        // code and its length for each literal; for each match length the
        // length code with its extra bits and their total size; the code and
        // its length for each distance symbol.
        let mut lit_tab = [0u32; 256];
        for (t, (&c, &l)) in lit_tab.iter_mut().zip(lit_codes.iter().zip(lit_lens)) {
            *t = u32::from(c) | (u32::from(l) << 16);
        }
        let mut len_tab = [0u32; 256];
        for (v, t) in len_tab.iter_mut().enumerate() {
            let code = LENGTH_CODE[v] as usize;
            let len_bits = u32::from(lit_lens[257 + code]);
            let extra = (v + MIN_MATCH - LENGTH_BASE[code] as usize) as u32;
            let value = u32::from(lit_codes[257 + code]) | (extra << len_bits);
            *t = (value << 8) | (len_bits + u32::from(LENGTH_EXTRA[code]));
        }
        let mut dist_tab = [0u32; 30];
        for (t, (&c, &l)) in dist_tab.iter_mut().zip(dist_codes.iter().zip(dist_lens)) {
            *t = u32::from(c) | (u32::from(l) << 16);
        }
        // Down to at most 7 bits before the first symbol.
        flush(&mut bits, &mut count, &mut at);
        for &sym in &self.syms[..self.nsyms] {
            let dist = (sym >> 16) as usize;
            let litlen = (sym & 0xFFFF) as usize;
            if dist == 0 {
                let e = lit_tab[litlen & 0xFF];
                bits |= u64::from(e & 0xFFFF) << count;
                count += e >> 16;
            } else {
                let e = len_tab[litlen & 0xFF];
                bits |= u64::from(e >> 8) << count;
                count += e & 0xFF;
                let d = dist_code(dist);
                let e = dist_tab[d];
                let dist_bits = e >> 16;
                let extra = (dist - DIST_BASE[d] as usize) as u64;
                bits |= (u64::from(e & 0xFFFF) | (extra << dist_bits)) << count;
                count += dist_bits + u32::from(DIST_EXTRA[d]);
            }
            flush(&mut bits, &mut count, &mut at);
        }
        bits |= u64::from(lit_codes[256]) << count;
        count += u32::from(lit_lens[256]);
        flush(&mut bits, &mut count, &mut at);
        out.truncate(at);
        // The rest of the writer keeps fewer than 32 bits in the buffer.
        self.bitbuf = bits;
        self.bitcnt = count;
    }
}

/// Four bytes of `win` at `pos`, little-endian (the window is padded).
#[inline(always)]
fn load32(win: &[u8], pos: usize) -> u32 {
    let mut b = [0u8; 4];
    b.copy_from_slice(&win[pos..pos + 4]);
    u32::from_le_bytes(b)
}

/// Hash of the low three bytes of `v` into `32 - shift` bits.
#[inline(always)]
fn hash3(v: u32, shift: u32) -> usize {
    hash(v, HASH3, shift)
}

/// Hashes the bytes of `v` that `mask` keeps.
#[inline(always)]
fn hash(v: u32, mask: u32, shift: u32) -> usize {
    ((v & mask).wrapping_mul(0x9E37_79B1) >> shift) as usize
}

/// The lazy search's chain budget past `CHAIN_FREE` (D-065).
struct Budget {
    left: i64,
    /// Stream position up to which bytes have earned steps.
    earned_to: u64,
    /// Bytes the window has slid: a window position plus this is a stream
    /// position.
    base: u64,
    /// This search was granted steps beyond the free ones.
    granted: bool,
}

impl Budget {
    /// Earns what the bytes up to window position `s` brought, then grants
    /// up to `want` steps from the bank (0 when it is empty).
    #[cold]
    #[inline(never)]
    fn grant(&mut self, s: usize, want: usize) -> usize {
        let at = self.base + s as u64;
        let gained = at
            .saturating_sub(self.earned_to)
            .saturating_mul(CHAIN_BUDGET as u64);
        self.left = (self.left + i64::try_from(gained).unwrap_or(CHAIN_BANK)).min(CHAIN_BANK);
        self.earned_to = at;
        let grant = want.min(self.left.max(0) as usize);
        self.left -= grant as i64;
        self.granted = grant > 0;
        grant
    }

    /// Gives back steps granted but not taken.
    #[cold]
    #[inline(never)]
    fn refund(&mut self, unused: usize) {
        self.left += unused as i64;
        self.granted = false;
    }
}

/// Chain steps every lazy search may take without the budget.
const CHAIN_FREE: usize = 16;
/// Chain steps past `CHAIN_FREE` the lazy search earns per input byte.
/// Ordinary data uses far fewer (35 steps per byte in all for English text
/// at level 9, under 30 for every input tried at levels 4 to 7); input built
/// to fill every chain with short matches used 90 to 380 at levels 8 and 9
/// (D-065).
const CHAIN_BUDGET: i64 = 64;
/// Most chain steps banked: one 32 KiB window's worth, so a stretch that
/// needs long searches can still have them.
const CHAIN_BANK: i64 = CHAIN_BUDGET * 32 * 1024;

/// Room a block needs beyond 4 bytes per symbol: the block header bits,
/// the end-of-block code, and the eight bytes the writer's last store may
/// reach past the block.
const BLOCK_SLACK: usize = 32;

/// Three bytes hashed (levels 1 to 3).
const HASH3: u32 = 0x00FF_FFFF;
/// Four bytes hashed (levels 4 to 9).
const HASH4: u32 = 0xFFFF_FFFF;

/// The hash mask a level uses: four bytes where lazy matching searches
/// chains, since a three-byte prefix is too common to tell candidates apart
/// on binary data and a three-byte match is rarely worth it there (D-065).
const fn hash_mask(level: u8) -> u32 {
    if params(level).lazy_matching {
        HASH4
    } else {
        HASH3
    }
}

/// Bytes a mask hashes.
const fn hash_len(mask: u32) -> usize {
    if mask == HASH4 { 4 } else { MIN_MATCH }
}

/// Inserts `pos` into the chains; returns the previous head.
#[inline(always)]
fn insert_at(
    head: &mut [u16],
    prev: &mut [u16],
    win: &[u8],
    pos: usize,
    mask: u32,
    shift: u32,
    wmask: usize,
) -> usize {
    let h = hash(load32(win, pos), mask, shift);
    let old = head[h];
    prev[pos & wmask] = old;
    head[h] = pos as u16;
    usize::from(old)
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_deflater_wipe_zeroes_every_buffer() {
        let mut d = Deflater::new(Level::DEFAULT);
        let mut out = Vec::new();
        d.compress_vec(&b"plaintext history ".repeat(500), &mut out, Flush::None)
            .unwrap();
        assert!(d.win.iter().any(|&b| b != 0));
        d.wipe();
        assert!(d.win.iter().all(|&b| b == 0));
        assert!(d.head.iter().all(|&h| h == 0));
        assert!(d.prev.iter().all(|&h| h == 0));
        assert!(d.nsyms == 0 && d.pending.is_empty());
        assert!(d.scratch.lit_freq.iter().all(|&f| f == 0));
        assert_eq!((d.total_in, d.bitbuf), (0, 0));
        let mut again = Vec::new();
        d.compress_vec(b"after", &mut again, Flush::Finish).unwrap();
        assert_eq!(crate::deflate::decompress(&again).unwrap(), b"after");
    }

    /// Memory is fixed when the engine is built: compressing never grows
    /// it, at any level or window size, with output drained in large or tiny
    /// pieces. (Levels 2 to 9 used to grow by a third on data with far
    /// three-byte repeats: blocks could pile up in `pending`, each with
    /// room for 6 bytes per symbol.)
    #[test]
    fn test_deflater_memory_never_grows() {
        let mut x: u64 = 0x1234_5678_9abc_def1;
        let mut far3 = Vec::new();
        while far3.len() < 1 << 20 {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            far3.push(x as u8);
            if far3.len() > 32_000 && x % 3 == 0 {
                let at = far3.len() - 32_000 + (x as usize >> 40) % 700;
                let rep = [far3[at], far3[at + 1], far3[at + 2]];
                far3.extend_from_slice(&rep);
            }
        }
        let random: Vec<u8> = (0..300_000u32)
            .map(|i| (i.wrapping_mul(2_654_435_761) >> 13) as u8)
            .collect();
        let text = b"memory stays where it was built. ".repeat(10_000);
        for bits in [9u8, 12, 15] {
            for level in 0..=9 {
                for data in [&far3, &random, &text] {
                    for out_size in [7usize, 1 << 16] {
                        let mut d = Deflater::new(Level::new(level).unwrap())
                            .with_window_bits(bits)
                            .unwrap();
                        let built = d.memory_in_use();
                        let mut buf = vec![0u8; out_size];
                        let mut inp = &data[..];
                        loop {
                            let p = d.compress(inp, &mut buf, Flush::Finish).unwrap();
                            inp = &inp[p.consumed..];
                            if p.status == Status::Done {
                                break;
                            }
                        }
                        assert_eq!(
                            d.memory_in_use(),
                            built,
                            "level {level} bits {bits} out {out_size} input {}",
                            data.len()
                        );
                    }
                    let mut d = Deflater::new(Level::new(level).unwrap())
                        .with_window_bits(bits)
                        .unwrap();
                    let built = d.memory_in_use();
                    d.compress_vec(data, &mut Vec::new(), Flush::Finish)
                        .unwrap();
                    assert_eq!(d.memory_in_use(), built, "vec, level {level} bits {bits}");
                }
            }
        }
    }

    #[test]
    fn test_wipe_vec_keeps_length_and_zeroes_contents() {
        // Spare capacity cannot be read back without `unsafe`; the length
        // and the visible elements can.
        let mut v: Vec<u8> = Vec::with_capacity(64);
        v.extend_from_slice(&[7; 40]);
        v.truncate(10);
        crate::wipe_vec(&mut v);
        assert_eq!(v.len(), 10);
        assert!(v.iter().all(|&b| b == 0));
        assert!(v.capacity() >= 64);
    }
}
