//! zlib framing (RFC 1950): a two-byte header, raw DEFLATE, and an Adler-32
//! of the data.
//!
//! This is also HTTP's `deflate` content coding (RFC 9110 section
//! 8.4.1.2).
//!
//! ```
//! use compress_lib::zlib;
//!
//! let packed = zlib::compress(b"hello, hello, hello");
//! assert_eq!(zlib::decompress(&packed)?, b"hello, hello, hello");
//! # Ok::<(), compress_lib::Error>(())
//! ```

use alloc::vec::Vec;

use crate::{
    Error, ErrorKind, Level, Limits, Progress, Status,
    checksum::{Adler32, adler32},
    deflate::{Deflater, Flush, Inflater},
};

/// Compresses `data` into a complete zlib stream at level 6.
///
/// # Examples
///
/// ```
/// let packed = compress_lib::zlib::compress(b"data");
/// assert_eq!(&packed[..2], &[0x78, 0x9C]);
/// ```
#[must_use]
pub fn compress(data: &[u8]) -> Vec<u8> {
    compress_with(data, Level::DEFAULT)
}

/// Compresses `data` into a complete zlib stream at `level`.
///
/// Allocation failure aborts, as for any `Vec`; [`Encoder::compress_vec`]
/// reports it as `AllocationFailed` instead.
///
/// # Examples
///
/// ```
/// use compress_lib::{Level, zlib};
///
/// let packed = zlib::compress_with(b"data", Level::BEST);
/// assert_eq!(&packed[..2], &[0x78, 0xDA]);
/// assert_eq!(zlib::decompress(&packed)?, b"data");
/// # Ok::<(), compress_lib::Error>(())
/// ```
#[must_use]
pub fn compress_with(data: &[u8], level: Level) -> Vec<u8> {
    let mut encoder = Encoder::new(level);
    let mut out = Vec::with_capacity(data.len() / 2 + 64);
    let result = encoder.compress_vec(data, &mut out, Flush::Finish);
    crate::one_shot_or_abort(result, &out);
    out
}

/// Decompresses a complete zlib stream with the default [`Limits`].
///
/// # Errors
///
/// Any decoding error; `ChecksumMismatch` when the Adler-32 differs;
/// `DictionaryRequired` for streams that need a preset dictionary (use
/// [`Decoder::set_dictionary`]); `UnexpectedEnd`, `TrailingData`,
/// `OutputLimit` as for raw DEFLATE.
///
/// # Examples
///
/// ```
/// use compress_lib::{ErrorKind, zlib};
///
/// let mut packed = zlib::compress(b"data");
/// let n = packed.len();
/// packed[n - 1] ^= 1;
/// assert_eq!(zlib::decompress(&packed).unwrap_err().kind(), ErrorKind::ChecksumMismatch);
/// ```
pub fn decompress(data: &[u8]) -> Result<Vec<u8>, Error> {
    decompress_with(data, Limits::new())
}

/// Decompresses a complete zlib stream with the given limits.
///
/// # Errors
///
/// As [`decompress`].
///
/// # Examples
///
/// ```
/// use compress_lib::{ErrorKind, Limits, zlib};
///
/// let packed = zlib::compress(&[0u8; 10_000]);
/// let tight = Limits::new().with_max_output(100);
/// assert_eq!(zlib::decompress_with(&packed, tight).unwrap_err().kind(), ErrorKind::OutputLimit);
/// ```
pub fn decompress_with(data: &[u8], limits: Limits) -> Result<Vec<u8>, Error> {
    let mut decoder = Decoder::one_shot(limits);
    let mut out = Vec::new();
    let reserve = data.len().saturating_mul(4).clamp(64, 1 << 20);
    let cap = usize::try_from(limits.max_output()).unwrap_or(usize::MAX);
    if out.try_reserve(reserve.min(cap)).is_err() {
        return Err(Error::config(ErrorKind::AllocationFailed));
    }
    let p = decoder.finish_vec(data, &mut out)?;
    if p.consumed != data.len() {
        return Err(Error::new(ErrorKind::TrailingData, p.consumed as u64));
    }
    Ok(out)
}

/// Checks a zlib header: compression method 8, window at most 32 KiB,
/// header check value.
pub(crate) fn header_ok(cmf: u8, flg: u8) -> bool {
    cmf & 0x0F == 8 && cmf >> 4 <= 7 && ((u16::from(cmf) << 8) | u16::from(flg)) % 31 == 0
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum State {
    Header,
    DictId,
    /// The header named a dictionary that was not given: waiting for
    /// `set_dictionary`, with the header and the id taken.
    NeedDict,
    Body,
    Trailer,
    Done,
}

/// A streaming zlib decoder.
///
/// The same shape as [`Inflater`]; the header is checked as it arrives, and
/// the Adler-32 trailer is verified before `Done` is returned.
///
/// # Examples
///
/// ```
/// use compress_lib::{deflate::Status, zlib};
///
/// let packed = zlib::compress(b"streamed");
/// let mut decoder = zlib::Decoder::new();
/// let mut out = [0u8; 32];
/// let p = decoder.finish(&packed, &mut out)?;
/// assert_eq!(p.status, Status::Done);
/// assert_eq!(&out[..p.produced], b"streamed");
/// # Ok::<(), compress_lib::Error>(())
/// ```
pub struct Decoder {
    inflater: Inflater,
    state: State,
    /// Header or trailer bytes collected so far.
    held: [u8; 4],
    held_len: usize,
    adler: Adler32,
    dictionary: Option<Vec<u8>>,
    dictionary_id: Option<u32>,
    max_window_bits: u8,
    consumed_total: u64,
    failed: Option<Error>,
}

impl core::fmt::Debug for Decoder {
    /// The preset dictionary is shown by length only: it may be secret.
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Decoder")
            .field("inflater", &self.inflater)
            .field("state", &self.state)
            .field("dictionary_len", &self.dictionary.as_ref().map(Vec::len))
            .field("dictionary_id", &self.dictionary_id)
            .field("max_window_bits", &self.max_window_bits)
            .field("total_in", &self.consumed_total)
            .field("failed", &self.failed)
            .finish_non_exhaustive()
    }
}

impl Default for Decoder {
    fn default() -> Self {
        Self::new()
    }
}

impl Decoder {
    /// A decoder accepting windows up to 32 KiB, with the default limits.
    ///
    /// # Examples
    ///
    /// ```
    /// let decoder = compress_lib::zlib::Decoder::new();
    /// assert!(!decoder.is_done());
    /// ```
    #[must_use]
    pub fn new() -> Self {
        Self::from_inflater(Inflater::new(), 15)
    }

    pub(crate) fn one_shot(limits: Limits) -> Self {
        Self::from_inflater(Inflater::without_history(limits), 15)
    }

    fn from_inflater(inflater: Inflater, max_window_bits: u8) -> Self {
        Self {
            inflater,
            state: State::Header,
            held: [0; 4],
            held_len: 0,
            adler: Adler32::new(),
            dictionary: None,
            dictionary_id: None,
            max_window_bits,
            consumed_total: 0,
            failed: None,
        }
    }

    /// Accepts windows up to `2^bits` bytes (8 to 15); a header naming a
    /// larger one fails with `InvalidWindowBits`. The decoder's own window
    /// shrinks with it.
    ///
    /// # Errors
    ///
    /// [`ErrorKind::InvalidWindowBits`] outside 8 to 15.
    ///
    /// # Examples
    ///
    /// ```
    /// use compress_lib::{ErrorKind, Level, zlib};
    ///
    /// let packed = zlib::compress(b"data"); // window bits 15
    /// let mut small = zlib::Decoder::new().with_window_bits(9)?;
    /// let err = small.finish(&packed, &mut [0u8; 16]).unwrap_err();
    /// assert_eq!(err.kind(), ErrorKind::InvalidWindowBits);
    /// # Ok::<(), compress_lib::Error>(())
    /// ```
    pub fn with_window_bits(mut self, bits: u8) -> Result<Self, Error> {
        self.inflater = self.inflater.with_window_bits(bits)?;
        self.max_window_bits = bits;
        Ok(self)
    }

    /// Sets the limits checked while decoding.
    ///
    /// # Examples
    ///
    /// ```
    /// use compress_lib::{Limits, zlib};
    ///
    /// let decoder = zlib::Decoder::new().with_limits(Limits::new().with_max_output(1 << 20));
    /// # let _ = decoder;
    /// ```
    #[must_use]
    pub fn with_limits(mut self, limits: Limits) -> Self {
        self.inflater = self.inflater.with_limits(limits);
        self
    }

    /// Gives the preset dictionary for streams that name one (FDICT). It is
    /// used only when the header asks for it, and only if its Adler-32
    /// matches the header's dictionary id.
    ///
    /// It can be given before the stream, or after a call returned
    /// [`ErrorKind::DictionaryRequired`]: that error does not fail the
    /// decoder. It has taken the header and the dictionary id
    /// ([`dictionary_id`](Decoder::dictionary_id) says which dictionary),
    /// and the error's [`offset`](Error::offset) is the number of stream
    /// bytes taken, so the next call continues with the input from there.
    ///
    /// # Errors
    ///
    /// [`ErrorKind::InvalidState`] after the body has started;
    /// [`ErrorKind::WrongDictionary`] after `DictionaryRequired` when the
    /// dictionary is not the one the header names (the decoder keeps
    /// waiting for the right one).
    ///
    /// # Examples
    ///
    /// ```
    /// use compress_lib::{Level, zlib};
    ///
    /// let dict = b"common words: hello world";
    /// let mut enc = zlib::Encoder::new(Level::BEST);
    /// enc.set_dictionary(dict)?;
    /// let mut packed = Vec::new();
    /// enc.compress_vec(b"hello world", &mut packed, compress_lib::deflate::Flush::Finish)?;
    ///
    /// let mut dec = zlib::Decoder::new();
    /// dec.set_dictionary(dict)?;
    /// let mut out = Vec::new();
    /// dec.finish_vec(&packed, &mut out)?;
    /// assert_eq!(out, b"hello world");
    ///
    /// // Or look the dictionary up by the id the stream names, then go on.
    /// let mut dec = zlib::Decoder::new();
    /// let mut out = Vec::new();
    /// let err = dec.finish_vec(&packed, &mut out).unwrap_err();
    /// assert_eq!(err.kind(), compress_lib::ErrorKind::DictionaryRequired);
    /// assert_eq!(dec.dictionary_id(), Some(compress_lib::checksum::adler32(dict)));
    /// dec.set_dictionary(dict)?;
    /// dec.finish_vec(&packed[err.offset() as usize..], &mut out)?;
    /// assert_eq!(out, b"hello world");
    /// # Ok::<(), compress_lib::Error>(())
    /// ```
    pub fn set_dictionary(&mut self, dictionary: &[u8]) -> Result<(), Error> {
        if !matches!(self.state, State::Header | State::DictId | State::NeedDict) {
            return Err(Error::config(ErrorKind::InvalidState));
        }
        if self.state == State::NeedDict && Some(adler32(dictionary)) != self.dictionary_id {
            return Err(Error::config(ErrorKind::WrongDictionary));
        }
        let mut stored = Vec::new();
        if stored.try_reserve_exact(dictionary.len()).is_err() {
            return Err(Error::config(ErrorKind::AllocationFailed));
        }
        stored.extend_from_slice(dictionary);
        self.dictionary = Some(stored);
        Ok(())
    }

    /// The dictionary id (Adler-32 of the dictionary) the header names, once
    /// the header has been read.
    ///
    /// # Examples
    ///
    /// ```
    /// use compress_lib::zlib;
    ///
    /// let mut dec = zlib::Decoder::new();
    /// dec.finish(&zlib::compress(b"x"), &mut [0u8; 4])?;
    /// assert_eq!(dec.dictionary_id(), None);
    /// # Ok::<(), compress_lib::Error>(())
    /// ```
    #[must_use]
    pub fn dictionary_id(&self) -> Option<u32> {
        self.dictionary_id
    }

    /// `true` once the trailer has been verified.
    ///
    /// # Examples
    ///
    /// ```
    /// use compress_lib::zlib;
    ///
    /// let mut dec = zlib::Decoder::new();
    /// dec.finish(&zlib::compress(b""), &mut [])?;
    /// assert!(dec.is_done());
    /// # Ok::<(), compress_lib::Error>(())
    /// ```
    #[must_use]
    pub fn is_done(&self) -> bool {
        self.state == State::Done
    }

    /// Compressed bytes consumed, header and trailer included.
    ///
    /// # Examples
    ///
    /// ```
    /// use compress_lib::zlib;
    ///
    /// let packed = zlib::compress(b"x");
    /// let mut dec = zlib::Decoder::new();
    /// dec.finish(&packed, &mut [0u8; 4])?;
    /// assert_eq!(dec.total_in(), packed.len() as u64);
    /// # Ok::<(), compress_lib::Error>(())
    /// ```
    #[must_use]
    pub fn total_in(&self) -> u64 {
        self.consumed_total
    }

    /// Decompressed bytes produced.
    ///
    /// # Examples
    ///
    /// ```
    /// use compress_lib::zlib;
    ///
    /// let mut dec = zlib::Decoder::new();
    /// dec.finish(&zlib::compress(b"four"), &mut [0u8; 8])?;
    /// assert_eq!(dec.total_out(), 4);
    /// # Ok::<(), compress_lib::Error>(())
    /// ```
    #[must_use]
    pub fn total_out(&self) -> u64 {
        self.inflater.total_out()
    }

    /// Starts over for a new stream, keeping allocations (and the
    /// dictionary, if one was given).
    ///
    /// # Examples
    ///
    /// ```
    /// use compress_lib::zlib;
    ///
    /// let mut dec = zlib::Decoder::new();
    /// dec.finish(&zlib::compress(b"a"), &mut [0u8; 4])?;
    /// dec.reset();
    /// dec.finish(&zlib::compress(b"b"), &mut [0u8; 4])?;
    /// # Ok::<(), compress_lib::Error>(())
    /// ```
    pub fn reset(&mut self) {
        self.inflater.reset();
        self.state = State::Header;
        self.held_len = 0;
        self.adler = Adler32::new();
        self.dictionary_id = None;
        self.consumed_total = 0;
        self.failed = None;
    }

    /// Zeroes the window, the decode tables, and the preset dictionary
    /// (which is dropped: unlike [`reset`](Decoder::reset), `wipe` forgets
    /// it), then resets the decoder. For connections that carried secrets.
    ///
    /// The zeroing goes through [`core::hint::black_box`]; see
    /// [`Inflater::wipe`](crate::deflate::Inflater::wipe) for what that can
    /// promise without `unsafe`.
    ///
    /// # Examples
    ///
    /// ```
    /// use compress_lib::zlib;
    ///
    /// let mut dec = zlib::Decoder::new();
    /// dec.set_dictionary(b"secret words")?;
    /// dec.finish_vec(&zlib::compress(b"token"), &mut Vec::new())?;
    /// dec.wipe();
    /// assert_eq!(dec.total_out(), 0);
    /// # Ok::<(), compress_lib::Error>(())
    /// ```
    pub fn wipe(&mut self) {
        self.inflater.wipe();
        if let Some(dict) = &mut self.dictionary {
            crate::wipe_vec(dict);
        }
        self.dictionary = None;
        self.held = [0; 4];
        self.reset();
    }

    /// Heap bytes held.
    ///
    /// # Examples
    ///
    /// ```
    /// assert!(compress_lib::zlib::Decoder::new().memory_in_use() > 32 * 1024);
    /// ```
    #[must_use]
    pub fn memory_in_use(&self) -> usize {
        self.inflater.memory_in_use() + self.dictionary.as_ref().map_or(0, Vec::capacity)
    }

    /// Decompresses as much of `input` into `output` as fits; more input may
    /// follow. As [`Inflater::decompress`], with `Done` only after the
    /// trailer is verified.
    ///
    /// # Errors
    ///
    /// Decoding errors, `InvalidHeader`, `InvalidWindowBits`,
    /// `DictionaryRequired`, `WrongDictionary`, `ChecksumMismatch`, and the
    /// limits.
    ///
    /// # Examples
    ///
    /// ```
    /// use compress_lib::{deflate::Status, zlib};
    ///
    /// let packed = zlib::compress(b"split");
    /// let mut dec = zlib::Decoder::new();
    /// let mut out = [0u8; 16];
    /// let a = dec.decompress(&packed[..3], &mut out)?;
    /// assert_eq!(a.status, Status::NeedsInput);
    /// let b = dec.decompress(&packed[3..], &mut out[a.produced..])?;
    /// assert_eq!(b.status, Status::Done);
    /// # Ok::<(), compress_lib::Error>(())
    /// ```
    pub fn decompress(&mut self, input: &[u8], output: &mut [u8]) -> Result<Progress, Error> {
        self.run(input, Sink::Slice(output), false)
    }

    /// As [`decompress`](Decoder::decompress), with `input` the last of the
    /// stream.
    ///
    /// # Errors
    ///
    /// As `decompress`, plus `UnexpectedEnd`.
    ///
    /// # Examples
    ///
    /// ```
    /// use compress_lib::{ErrorKind, zlib};
    ///
    /// let packed = zlib::compress(b"data");
    /// let err = zlib::Decoder::new().finish(&packed[..packed.len() - 1], &mut [0u8; 8]).unwrap_err();
    /// assert_eq!(err.kind(), ErrorKind::UnexpectedEnd);
    /// ```
    pub fn finish(&mut self, input: &[u8], output: &mut [u8]) -> Result<Progress, Error> {
        self.run(input, Sink::Slice(output), true)
    }

    /// Decompresses `input`, appending to `out`.
    ///
    /// # Errors
    ///
    /// As [`decompress`](Decoder::decompress), plus `AllocationFailed`.
    ///
    /// # Examples
    ///
    /// ```
    /// use compress_lib::zlib;
    ///
    /// let mut out = Vec::new();
    /// zlib::Decoder::new().decompress_vec(&zlib::compress(b"vec"), &mut out)?;
    /// assert_eq!(out, b"vec");
    /// # Ok::<(), compress_lib::Error>(())
    /// ```
    pub fn decompress_vec(&mut self, input: &[u8], out: &mut Vec<u8>) -> Result<Progress, Error> {
        self.run(input, Sink::Vec(out), false)
    }

    /// As [`decompress_vec`](Decoder::decompress_vec), with `input` the last
    /// of the stream.
    ///
    /// # Errors
    ///
    /// As `decompress_vec`, plus `UnexpectedEnd`.
    ///
    /// # Examples
    ///
    /// ```
    /// use compress_lib::zlib;
    ///
    /// let mut out = Vec::new();
    /// zlib::Decoder::new().finish_vec(&zlib::compress(b"vec"), &mut out)?;
    /// assert_eq!(out, b"vec");
    /// # Ok::<(), compress_lib::Error>(())
    /// ```
    pub fn finish_vec(&mut self, input: &[u8], out: &mut Vec<u8>) -> Result<Progress, Error> {
        self.run(input, Sink::Vec(out), true)
    }

    fn fail(&mut self, kind: ErrorKind, at: usize) -> Error {
        let e = Error::new(kind, self.consumed_total + at as u64);
        self.failed = Some(e);
        e
    }

    fn run(&mut self, input: &[u8], mut sink: Sink<'_>, last: bool) -> Result<Progress, Error> {
        if let Some(e) = self.failed {
            return Err(e);
        }
        let mut ip = 0;
        let mut produced = 0;
        let status = loop {
            match self.state {
                State::NeedDict => {
                    let Some(dict) = &self.dictionary else {
                        // Not a failure: the caller can still give it.
                        self.consumed_total += ip as u64;
                        return Err(Error::new(
                            ErrorKind::DictionaryRequired,
                            self.consumed_total,
                        ));
                    };
                    // `set_dictionary` checked it against the id.
                    self.inflater.set_dictionary(dict)?;
                    self.state = State::Body;
                }
                State::Header | State::DictId | State::Trailer => {
                    let need = if self.state == State::Header { 2 } else { 4 };
                    while self.held_len < need && ip < input.len() {
                        self.held[self.held_len] = input[ip];
                        self.held_len += 1;
                        ip += 1;
                    }
                    if self.held_len < need {
                        break Status::NeedsInput;
                    }
                    self.held_len = 0;
                    if let Err(kind) = self.on_field() {
                        if self.state == State::NeedDict {
                            continue;
                        }
                        return Err(self.fail(kind, ip));
                    }
                }
                State::Body => {
                    let r = match &mut sink {
                        Sink::Slice(out) => self
                            .inflater
                            .decompress(&input[ip..], &mut out[produced..])
                            .inspect(|p| {
                                self.adler.update(&out[produced..produced + p.produced]);
                            }),
                        Sink::Vec(out) => {
                            let at = out.len();
                            self.inflater
                                .decompress_vec(&input[ip..], out)
                                .inspect(|_| {
                                    self.adler.update(&out[at..]);
                                })
                        }
                    };
                    let p = match r {
                        Ok(p) => p,
                        Err(e) => {
                            self.failed = Some(e);
                            return Err(e);
                        }
                    };
                    ip += p.consumed;
                    produced += p.produced;
                    match p.status {
                        Status::Done => self.state = State::Trailer,
                        Status::NeedsOutput => break Status::NeedsOutput,
                        _ => break Status::NeedsInput,
                    }
                }
                State::Done => break Status::Done,
            }
        };
        self.consumed_total += ip as u64;
        if status == Status::NeedsInput && last {
            return Err(self.fail(ErrorKind::UnexpectedEnd, 0));
        }
        Ok(Progress::new(ip, produced, status))
    }

    /// Handles a completed header, dictionary id, or trailer.
    fn on_field(&mut self) -> Result<(), ErrorKind> {
        match self.state {
            State::Header => {
                let (cmf, flg) = (self.held[0], self.held[1]);
                if !header_ok(cmf, flg) {
                    return Err(ErrorKind::InvalidHeader);
                }
                if (cmf >> 4) + 8 > self.max_window_bits {
                    return Err(ErrorKind::InvalidWindowBits);
                }
                self.state = if flg & 0x20 != 0 {
                    State::DictId
                } else {
                    State::Body
                };
            }
            State::DictId => {
                let id = u32::from_be_bytes(self.held);
                self.dictionary_id = Some(id);
                let Some(dict) = &self.dictionary else {
                    self.state = State::NeedDict;
                    return Err(ErrorKind::DictionaryRequired);
                };
                if adler32(dict) != id {
                    return Err(ErrorKind::WrongDictionary);
                }
                self.inflater.set_dictionary(dict).map_err(|e| e.kind())?;
                self.state = State::Body;
            }
            State::Trailer => {
                if u32::from_be_bytes(self.held) != self.adler.finish() {
                    return Err(ErrorKind::ChecksumMismatch);
                }
                self.state = State::Done;
            }
            State::NeedDict | State::Body | State::Done => {}
        }
        Ok(())
    }
}

/// Where decoded bytes go.
pub(crate) enum Sink<'a> {
    Slice(&'a mut [u8]),
    Vec(&'a mut Vec<u8>),
}

/// A streaming zlib encoder.
///
/// # Examples
///
/// ```
/// use compress_lib::{Level, deflate::Flush, zlib};
///
/// let mut enc = zlib::Encoder::new(Level::DEFAULT);
/// let mut packed = Vec::new();
/// enc.compress_vec(b"part one, ", &mut packed, Flush::None)?;
/// enc.compress_vec(b"part two", &mut packed, Flush::Finish)?;
/// assert_eq!(zlib::decompress(&packed)?, b"part one, part two");
/// # Ok::<(), compress_lib::Error>(())
/// ```
pub struct Encoder {
    deflater: Deflater,
    adler: Adler32,
    dictionary_id: Option<u32>,
    /// Header (then trailer) bytes not yet handed out.
    pending: [u8; 6],
    pending_start: usize,
    pending_len: usize,
    header_written: bool,
    trailer_written: bool,
}

impl core::fmt::Debug for Encoder {
    /// The header and trailer bytes in waiting (the trailer is a checksum
    /// of the plaintext) are not shown.
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Encoder")
            .field("deflater", &self.deflater)
            .field("dictionary_id", &self.dictionary_id)
            .field("header_written", &self.header_written)
            .field("trailer_written", &self.trailer_written)
            .finish_non_exhaustive()
    }
}

impl Default for Encoder {
    fn default() -> Self {
        Self::new(Level::DEFAULT)
    }
}

impl Encoder {
    /// An encoder at `level` with window bits 15.
    ///
    /// # Examples
    ///
    /// ```
    /// let enc = compress_lib::zlib::Encoder::new(compress_lib::Level::FAST);
    /// # let _ = enc;
    /// ```
    #[must_use]
    pub fn new(level: Level) -> Self {
        Self {
            deflater: Deflater::new(level),
            adler: Adler32::new(),
            dictionary_id: None,
            pending: [0; 6],
            pending_start: 0,
            pending_len: 0,
            header_written: false,
            trailer_written: false,
        }
    }

    /// Sets the window to `2^bits` bytes, 8 to 15 (the header names it;
    /// zlib's own encoder rounds 8 up to 9, this one does not).
    ///
    /// # Errors
    ///
    /// [`ErrorKind::InvalidWindowBits`] outside 8 to 15.
    ///
    /// # Examples
    ///
    /// ```
    /// use compress_lib::{Level, deflate::Flush, zlib};
    ///
    /// let mut enc = zlib::Encoder::new(Level::DEFAULT).with_window_bits(10)?;
    /// let mut packed = Vec::new();
    /// enc.compress_vec(b"x", &mut packed, Flush::Finish)?;
    /// assert_eq!(packed[0], 0x28);
    /// # Ok::<(), compress_lib::Error>(())
    /// ```
    pub fn with_window_bits(mut self, bits: u8) -> Result<Self, Error> {
        self.deflater = self.deflater.with_window_bits(bits)?;
        Ok(self)
    }

    /// Presets a dictionary; the header then names it (FDICT) and the
    /// decoder must be given the same one. Only before any input.
    ///
    /// # Errors
    ///
    /// [`ErrorKind::InvalidState`] after input has been given.
    ///
    /// # Examples
    ///
    /// ```
    /// let mut enc = compress_lib::zlib::Encoder::default();
    /// enc.set_dictionary(b"dictionary")?;
    /// # Ok::<(), compress_lib::Error>(())
    /// ```
    pub fn set_dictionary(&mut self, dictionary: &[u8]) -> Result<(), Error> {
        if self.header_written {
            return Err(Error::config(ErrorKind::InvalidState));
        }
        self.deflater.set_dictionary(dictionary)?;
        self.dictionary_id = Some(adler32(dictionary));
        Ok(())
    }

    /// Starts a new stream, keeping allocations (and no dictionary).
    ///
    /// # Examples
    ///
    /// ```
    /// let mut enc = compress_lib::zlib::Encoder::default();
    /// enc.reset();
    /// ```
    pub fn reset(&mut self) {
        self.deflater.reset();
        self.adler = Adler32::new();
        self.dictionary_id = None;
        self.pending_start = 0;
        self.pending_len = 0;
        self.header_written = false;
        self.trailer_written = false;
    }

    /// Zeroes the window, hash chains, and buffers (they hold plaintext and
    /// any preset dictionary), then resets the encoder. For connections that
    /// carried secrets.
    ///
    /// The zeroing goes through [`core::hint::black_box`]; see
    /// [`Inflater::wipe`](crate::deflate::Inflater::wipe) for what that can
    /// promise without `unsafe`.
    ///
    /// # Examples
    ///
    /// ```
    /// use compress_lib::{Level, deflate::Flush, zlib};
    ///
    /// let mut enc = zlib::Encoder::new(Level::DEFAULT);
    /// enc.compress_vec(b"token=secret", &mut Vec::new(), Flush::Finish)?;
    /// enc.wipe();
    /// assert_eq!(enc.total_in(), 0);
    /// # Ok::<(), compress_lib::Error>(())
    /// ```
    pub fn wipe(&mut self) {
        self.deflater.wipe();
        self.pending = [0; 6];
        core::hint::black_box(&self.pending);
        self.reset();
    }

    /// Uncompressed bytes taken.
    ///
    /// # Examples
    ///
    /// ```
    /// use compress_lib::{deflate::Flush, zlib};
    ///
    /// let mut enc = zlib::Encoder::default();
    /// enc.compress_vec(b"abc", &mut Vec::new(), Flush::Finish)?;
    /// assert_eq!(enc.total_in(), 3);
    /// # Ok::<(), compress_lib::Error>(())
    /// ```
    #[must_use]
    pub fn total_in(&self) -> u64 {
        self.deflater.total_in()
    }

    /// Heap bytes held.
    ///
    /// # Examples
    ///
    /// ```
    /// assert!(compress_lib::zlib::Encoder::default().memory_in_use() > 0);
    /// ```
    #[must_use]
    pub fn memory_in_use(&self) -> usize {
        self.deflater.memory_in_use()
    }

    fn header(&self) -> [u8; 2] {
        let cmf = ((self.deflater.window_bits() - 8) << 4) | 8;
        let level_bits = match self.deflater.level().get() {
            0 | 1 => 0,
            2..=5 => 1,
            6 => 2,
            _ => 3,
        };
        let mut flg = (level_bits << 6)
            | if self.dictionary_id.is_some() {
                0x20
            } else {
                0
            };
        let rem = ((u16::from(cmf) << 8) | u16::from(flg)) % 31;
        if rem != 0 {
            flg += (31 - rem) as u8;
        }
        [cmf, flg]
    }

    fn drain(&mut self, output: &mut [u8], produced: &mut usize) {
        let n = self.pending_len.min(output.len() - *produced);
        output[*produced..*produced + n]
            .copy_from_slice(&self.pending[self.pending_start..self.pending_start + n]);
        self.pending_start += n;
        self.pending_len -= n;
        *produced += n;
    }

    /// As [`Deflater::compress`], with the header written first and the
    /// Adler-32 trailer after `Finish`.
    ///
    /// # Errors
    ///
    /// As `Deflater::compress`.
    ///
    /// # Examples
    ///
    /// ```
    /// use compress_lib::{deflate::{Flush, Status}, zlib};
    ///
    /// let mut enc = zlib::Encoder::default();
    /// let mut out = [0u8; 64];
    /// let p = enc.compress(b"abc", &mut out, Flush::Finish)?;
    /// assert_eq!(p.status, Status::Done);
    /// assert_eq!(zlib::decompress(&out[..p.produced])?, b"abc");
    /// # Ok::<(), compress_lib::Error>(())
    /// ```
    pub fn compress(
        &mut self,
        input: &[u8],
        output: &mut [u8],
        flush: Flush,
    ) -> Result<Progress, Error> {
        let mut produced = 0;
        if !self.header_written {
            let h = self.header();
            self.pending[..2].copy_from_slice(&h);
            if let Some(id) = self.dictionary_id {
                self.pending[2..6].copy_from_slice(&id.to_be_bytes());
                self.pending_len = 6;
            } else {
                self.pending_len = 2;
            }
            self.pending_start = 0;
            self.header_written = true;
        }
        self.drain(output, &mut produced);
        if self.pending_len > 0 {
            return Ok(Progress::new(0, produced, Status::NeedsOutput));
        }
        if self.trailer_written {
            if !input.is_empty() {
                return Err(Error::config(ErrorKind::InvalidState));
            }
            return Ok(Progress::new(0, produced, Status::Done));
        }
        let p = self
            .deflater
            .compress(input, &mut output[produced..], flush)?;
        self.adler.update(&input[..p.consumed]);
        produced += p.produced;
        if p.status == Status::Done && flush == Flush::Finish {
            self.pending[..4].copy_from_slice(&self.adler.finish().to_be_bytes());
            self.pending_start = 0;
            self.pending_len = 4;
            self.trailer_written = true;
            self.drain(output, &mut produced);
            let status = if self.pending_len == 0 {
                Status::Done
            } else {
                Status::NeedsOutput
            };
            return Ok(Progress::new(p.consumed, produced, status));
        }
        Ok(Progress::new(p.consumed, produced, p.status))
    }

    /// As [`compress`](Encoder::compress), appending to `out` until the
    /// flush point is written.
    ///
    /// # Errors
    ///
    /// As `compress`, plus `AllocationFailed`.
    ///
    /// # Examples
    ///
    /// ```
    /// use compress_lib::{deflate::Flush, zlib};
    ///
    /// let mut out = Vec::new();
    /// zlib::Encoder::default().compress_vec(b"abc", &mut out, Flush::Finish)?;
    /// assert_eq!(zlib::decompress(&out)?, b"abc");
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
            match self.compress(&input[consumed..], &mut out[have..], flush) {
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
}

#[cfg(test)]
mod tests {
    use alloc::vec;

    use super::*;

    #[test]
    fn test_zlib_header_values_match_zlib() {
        assert_eq!(&compress_with(b"", Level::NONE)[..2], &[0x78, 0x01]);
        assert_eq!(&compress_with(b"", Level::FAST)[..2], &[0x78, 0x01]);
        assert_eq!(
            &compress_with(b"", Level::new(4).unwrap())[..2],
            &[0x78, 0x5E]
        );
        assert_eq!(&compress(b"")[..2], &[0x78, 0x9C]);
        assert_eq!(&compress_with(b"", Level::BEST)[..2], &[0x78, 0xDA]);
    }

    #[test]
    fn test_zlib_known_stream_from_python_zlib() {
        // zlib.compress(b"hello") from CPython's zlib 1.3.
        let packed = [
            0x78, 0x9C, 0xCB, 0x48, 0xCD, 0xC9, 0xC9, 0x07, 0x00, 0x06, 0x2C, 0x02, 0x15,
        ];
        assert_eq!(decompress(&packed).unwrap(), b"hello");
    }

    #[test]
    fn test_zlib_bad_headers_rejected() {
        // Method 7, window 64 KiB (CINFO 8), bad check value.
        assert_eq!(
            decompress(&[0x77, 0x01, 0x03, 0x00]).unwrap_err().kind(),
            ErrorKind::InvalidHeader
        );
        assert_eq!(
            decompress(&[0x88, 0x1C, 0x03, 0x00]).unwrap_err().kind(),
            ErrorKind::InvalidHeader
        );
        assert_eq!(
            decompress(&[0x78, 0x9D, 0x03, 0x00]).unwrap_err().kind(),
            ErrorKind::InvalidHeader
        );
    }

    #[test]
    fn test_zlib_dictionary_required_and_wrong() {
        let mut enc = Encoder::new(Level::BEST);
        enc.set_dictionary(b"the dictionary").unwrap();
        let mut packed = Vec::new();
        enc.compress_vec(b"the dictionary again", &mut packed, Flush::Finish)
            .unwrap();
        assert_eq!(
            decompress(&packed).unwrap_err().kind(),
            ErrorKind::DictionaryRequired
        );
        let mut dec = Decoder::new();
        dec.set_dictionary(b"another one").unwrap();
        assert_eq!(
            dec.finish_vec(&packed, &mut Vec::new()).unwrap_err().kind(),
            ErrorKind::WrongDictionary
        );
        let mut dec = Decoder::new();
        dec.set_dictionary(b"the dictionary").unwrap();
        let mut out = Vec::new();
        dec.finish_vec(&packed, &mut out).unwrap();
        assert_eq!(out, b"the dictionary again");
        assert_eq!(dec.dictionary_id(), Some(adler32(b"the dictionary")));
    }

    /// `DictionaryRequired` is not a failure: the caller reads the id, gives
    /// the dictionary, and goes on from the error's offset, in one piece or
    /// byte by byte. A wrong dictionary is refused and the decoder keeps
    /// waiting. (It used to be sticky, and `set_dictionary` returned `Ok`
    /// without effect.)
    #[test]
    fn test_zlib_dictionary_required_then_continue() {
        let dict = b"the dictionary";
        let data = b"the dictionary, then the dictionary once more";
        let mut enc = Encoder::new(Level::BEST);
        enc.set_dictionary(dict).unwrap();
        let mut packed = Vec::new();
        enc.compress_vec(data, &mut packed, Flush::Finish).unwrap();

        let mut dec = Decoder::new();
        let mut out = Vec::new();
        let err = dec.finish_vec(&packed, &mut out).unwrap_err();
        assert_eq!(err.kind(), ErrorKind::DictionaryRequired);
        assert_eq!(err.offset(), 6, "header and id taken");
        assert_eq!(dec.dictionary_id(), Some(adler32(dict)));
        // Asking again without a dictionary gives the same answer.
        assert_eq!(
            dec.finish_vec(&packed[6..], &mut out).unwrap_err().kind(),
            ErrorKind::DictionaryRequired
        );
        assert_eq!(
            dec.set_dictionary(b"not it").unwrap_err().kind(),
            ErrorKind::WrongDictionary
        );
        dec.set_dictionary(dict).unwrap();
        let p = dec.finish_vec(&packed[6..], &mut out).unwrap();
        assert_eq!(p.status, Status::Done);
        assert_eq!(out, data);

        // Byte by byte through the slice API.
        let mut dec = Decoder::new();
        let mut out = Vec::new();
        let mut buf = [0u8; 3];
        let mut pos = 0;
        while pos < packed.len() {
            let last = pos + 1 == packed.len();
            let piece = &packed[pos..=pos];
            let r = if last {
                dec.finish(piece, &mut buf)
            } else {
                dec.decompress(piece, &mut buf)
            };
            match r {
                Ok(p) => {
                    out.extend_from_slice(&buf[..p.produced]);
                    pos += p.consumed;
                    if p.status == Status::Done {
                        break;
                    }
                }
                Err(e) => {
                    assert_eq!(e.kind(), ErrorKind::DictionaryRequired);
                    assert_eq!(e.offset(), 6);
                    dec.set_dictionary(dict).unwrap();
                    pos = 6;
                }
            }
        }
        assert_eq!(out, data);
        assert!(dec.is_done());
    }

    #[test]
    fn test_zlib_every_split_and_buffer() {
        let data: Vec<u8> = (0..3000u32).map(|i| (i % 97) as u8).collect();
        let packed = compress(&data);
        for step in [1usize, 2, 5, 13] {
            for out_size in [1usize, 3, 64] {
                let mut dec = Decoder::new();
                let mut out = Vec::new();
                let mut buf = vec![0u8; out_size];
                let mut pos = 0;
                loop {
                    let end = (pos + step).min(packed.len());
                    let p = if end == packed.len() {
                        dec.finish(&packed[pos..end], &mut buf).unwrap()
                    } else {
                        dec.decompress(&packed[pos..end], &mut buf).unwrap()
                    };
                    out.extend_from_slice(&buf[..p.produced]);
                    pos += p.consumed;
                    if p.status == Status::Done {
                        break;
                    }
                }
                assert_eq!(out, data, "{step}/{out_size}");
            }
        }
    }

    #[test]
    fn test_zlib_trailer_and_truncation() {
        let packed = compress(b"checked");
        for n in 0..packed.len() {
            assert_eq!(
                decompress(&packed[..n]).unwrap_err().kind(),
                ErrorKind::UnexpectedEnd,
                "prefix {n}"
            );
        }
        let mut extra = packed.clone();
        extra.push(0);
        assert_eq!(
            decompress(&extra).unwrap_err().kind(),
            ErrorKind::TrailingData
        );
        let mut bad = packed;
        let n = bad.len();
        bad[n - 2] ^= 0x10;
        assert_eq!(
            decompress(&bad).unwrap_err().kind(),
            ErrorKind::ChecksumMismatch
        );
    }
}
