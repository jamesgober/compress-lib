//! gzip framing (RFC 1952): a header, raw DEFLATE, and a CRC-32 and length
//! of the data; several members may follow one another.
//!
//! ```
//! use compress_lib::gzip;
//!
//! let packed = gzip::compress(b"hello, hello, hello");
//! assert_eq!(gzip::decompress(&packed)?, b"hello, hello, hello");
//! # Ok::<(), compress_lib::Error>(())
//! ```

use alloc::vec::Vec;

use crate::{
    Error, ErrorKind, Level, Limits, Progress, Status,
    checksum::Crc32,
    deflate::{Deflater, Flush, Inflater},
    zlib::Sink,
};

const ID1: u8 = 0x1F;
const ID2: u8 = 0x8B;
const FTEXT: u8 = 1;
const FHCRC: u8 = 2;
const FEXTRA: u8 = 4;
const FNAME: u8 = 8;
const FCOMMENT: u8 = 16;
/// Operating system "unknown": the same bytes on every platform.
const OS_UNKNOWN: u8 = 255;

/// Compresses `data` into a gzip member at level 6, with a header that has
/// no name, no time stamp, and OS "unknown" (the same bytes everywhere).
///
/// # Examples
///
/// ```
/// let packed = compress_lib::gzip::compress(b"data");
/// assert_eq!(&packed[..3], &[0x1F, 0x8B, 0x08]);
/// ```
#[must_use]
pub fn compress(data: &[u8]) -> Vec<u8> {
    compress_with(data, Level::DEFAULT)
}

/// Compresses `data` into a gzip member at `level`.
///
/// Allocation failure aborts, as for any `Vec`; [`Encoder::compress_vec`]
/// reports it as `AllocationFailed` instead.
///
/// # Examples
///
/// ```
/// use compress_lib::{Level, gzip};
///
/// let packed = gzip::compress_with(b"data", Level::FAST);
/// assert_eq!(gzip::decompress(&packed)?, b"data");
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

/// Decompresses a complete gzip stream (all members) with the default
/// [`Limits`].
///
/// # Errors
///
/// Any decoding error; `InvalidHeader`, `ChecksumMismatch`,
/// `LengthMismatch`, `HeaderTooLarge`, `TooManyMembers`; `UnexpectedEnd`,
/// `TrailingData`, `OutputLimit` as for raw DEFLATE.
///
/// # Examples
///
/// ```
/// use compress_lib::gzip;
///
/// let mut two = gzip::compress(b"first ");
/// two.extend_from_slice(&gzip::compress(b"second"));
/// assert_eq!(gzip::decompress(&two)?, b"first second");
/// # Ok::<(), compress_lib::Error>(())
/// ```
pub fn decompress(data: &[u8]) -> Result<Vec<u8>, Error> {
    decompress_with(data, Limits::new())
}

/// Decompresses a complete gzip stream with the given limits.
///
/// # Errors
///
/// As [`decompress`].
///
/// # Examples
///
/// ```
/// use compress_lib::{ErrorKind, Limits, gzip};
///
/// let packed = gzip::compress(&[0u8; 10_000]);
/// let tight = Limits::new().with_max_output(100);
/// assert_eq!(gzip::decompress_with(&packed, tight).unwrap_err().kind(), ErrorKind::OutputLimit);
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

/// The fields of a gzip header.
///
/// Decoders fill it only when asked ([`Decoder::with_keep_header`]); the
/// optional fields are bounded by [`Limits::max_header`].
///
/// # Examples
///
/// ```
/// use compress_lib::{Level, deflate::Flush, gzip::{self, GzipHeader}};
///
/// let header = GzipHeader::new().with_name(b"report.csv").with_mtime(1_700_000_000);
/// let mut enc = gzip::Encoder::new(Level::DEFAULT).with_header(header);
/// let mut packed = Vec::new();
/// enc.compress_vec(b"a,b\n1,2\n", &mut packed, Flush::Finish)?;
///
/// let mut dec = gzip::Decoder::new().with_keep_header(true);
/// let mut out = Vec::new();
/// dec.finish_vec(&packed, &mut out)?;
/// assert_eq!(dec.header().and_then(|h| h.name()), Some(&b"report.csv"[..]));
/// # Ok::<(), compress_lib::Error>(())
/// ```
#[derive(Clone, Debug, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct GzipHeader {
    mtime: u32,
    os: u8,
    extra_flags: u8,
    text: bool,
    name: Option<Vec<u8>>,
    comment: Option<Vec<u8>>,
    extra: Option<Vec<u8>>,
}

impl GzipHeader {
    /// Zeroes the name, comment, and extra field (spare capacity included).
    pub(crate) fn wipe(&mut self) {
        for v in [&mut self.name, &mut self.comment, &mut self.extra]
            .into_iter()
            .flatten()
        {
            crate::wipe_vec(v);
        }
        *self = Self::default();
    }

    /// No name, comment, or extra field; time stamp 0; OS "unknown".
    ///
    /// # Examples
    ///
    /// ```
    /// assert_eq!(compress_lib::gzip::GzipHeader::new().os(), 255);
    /// ```
    #[must_use]
    pub fn new() -> Self {
        Self {
            os: OS_UNKNOWN,
            ..Self::default()
        }
    }

    /// Sets the file name (written without its terminating zero; it must
    /// not contain one).
    ///
    /// # Examples
    ///
    /// ```
    /// let h = compress_lib::gzip::GzipHeader::new().with_name(b"a.txt");
    /// assert_eq!(h.name(), Some(&b"a.txt"[..]));
    /// ```
    #[must_use]
    pub fn with_name(mut self, name: &[u8]) -> Self {
        self.name = Some(name.iter().copied().take_while(|&b| b != 0).collect());
        self
    }

    /// Sets the comment (it must not contain a zero byte).
    ///
    /// # Examples
    ///
    /// ```
    /// let h = compress_lib::gzip::GzipHeader::new().with_comment(b"nightly export");
    /// assert_eq!(h.comment(), Some(&b"nightly export"[..]));
    /// ```
    #[must_use]
    pub fn with_comment(mut self, comment: &[u8]) -> Self {
        self.comment = Some(comment.iter().copied().take_while(|&b| b != 0).collect());
        self
    }

    /// Sets the extra field (at most 65,535 bytes; longer is cut).
    ///
    /// # Examples
    ///
    /// ```
    /// let h = compress_lib::gzip::GzipHeader::new().with_extra(b"AB\x02\x00hi");
    /// assert_eq!(h.extra().map(<[u8]>::len), Some(6));
    /// ```
    #[must_use]
    pub fn with_extra(mut self, extra: &[u8]) -> Self {
        self.extra = Some(extra[..extra.len().min(0xFFFF)].to_vec());
        self
    }

    /// Sets the modification time (Unix seconds; 0 means none).
    ///
    /// # Examples
    ///
    /// ```
    /// assert_eq!(compress_lib::gzip::GzipHeader::new().with_mtime(7).mtime(), 7);
    /// ```
    #[must_use]
    pub fn with_mtime(mut self, mtime: u32) -> Self {
        self.mtime = mtime;
        self
    }

    /// Sets the operating system byte (RFC 1952 section 2.3.1).
    ///
    /// # Examples
    ///
    /// ```
    /// assert_eq!(compress_lib::gzip::GzipHeader::new().with_os(3).os(), 3);
    /// ```
    #[must_use]
    pub fn with_os(mut self, os: u8) -> Self {
        self.os = os;
        self
    }

    /// Marks the data as probably text (FTEXT).
    ///
    /// # Examples
    ///
    /// ```
    /// assert!(compress_lib::gzip::GzipHeader::new().with_text(true).text());
    /// ```
    #[must_use]
    pub fn with_text(mut self, text: bool) -> Self {
        self.text = text;
        self
    }

    /// The file name, if any.
    ///
    /// # Examples
    ///
    /// ```
    /// assert_eq!(compress_lib::gzip::GzipHeader::new().name(), None);
    /// ```
    #[must_use]
    pub fn name(&self) -> Option<&[u8]> {
        self.name.as_deref()
    }

    /// The comment, if any.
    ///
    /// # Examples
    ///
    /// ```
    /// assert_eq!(compress_lib::gzip::GzipHeader::new().comment(), None);
    /// ```
    #[must_use]
    pub fn comment(&self) -> Option<&[u8]> {
        self.comment.as_deref()
    }

    /// The extra field, if any.
    ///
    /// # Examples
    ///
    /// ```
    /// assert_eq!(compress_lib::gzip::GzipHeader::new().extra(), None);
    /// ```
    #[must_use]
    pub fn extra(&self) -> Option<&[u8]> {
        self.extra.as_deref()
    }

    /// The modification time.
    ///
    /// # Examples
    ///
    /// ```
    /// assert_eq!(compress_lib::gzip::GzipHeader::new().mtime(), 0);
    /// ```
    #[must_use]
    pub fn mtime(&self) -> u32 {
        self.mtime
    }

    /// The operating system byte.
    ///
    /// # Examples
    ///
    /// ```
    /// assert_eq!(compress_lib::gzip::GzipHeader::new().os(), 255);
    /// ```
    #[must_use]
    pub fn os(&self) -> u8 {
        self.os
    }

    /// The extra flags byte (2: best compression, 4: fastest).
    ///
    /// # Examples
    ///
    /// ```
    /// assert_eq!(compress_lib::gzip::GzipHeader::new().extra_flags(), 0);
    /// ```
    #[must_use]
    pub fn extra_flags(&self) -> u8 {
        self.extra_flags
    }

    /// Whether FTEXT is set.
    ///
    /// # Examples
    ///
    /// ```
    /// assert!(!compress_lib::gzip::GzipHeader::new().text());
    /// ```
    #[must_use]
    pub fn text(&self) -> bool {
        self.text
    }

    /// The encoded header (with `extra_flags` from the level).
    fn encode(&self, xfl: u8) -> Vec<u8> {
        let mut flg = 0;
        if self.text {
            flg |= FTEXT;
        }
        if self.extra.is_some() {
            flg |= FEXTRA;
        }
        if self.name.is_some() {
            flg |= FNAME;
        }
        if self.comment.is_some() {
            flg |= FCOMMENT;
        }
        let mut h = Vec::with_capacity(10);
        h.extend_from_slice(&[ID1, ID2, 8, flg]);
        h.extend_from_slice(&self.mtime.to_le_bytes());
        h.extend_from_slice(&[xfl, self.os]);
        if let Some(extra) = &self.extra {
            h.extend_from_slice(&(extra.len() as u16).to_le_bytes());
            h.extend_from_slice(extra);
        }
        for field in [&self.name, &self.comment].into_iter().flatten() {
            h.extend_from_slice(field);
            h.push(0);
        }
        h
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum State {
    /// Fixed ten-byte header: `index` bytes read.
    Fixed,
    ExtraLength,
    Extra,
    Name,
    Comment,
    HeaderCrc,
    Body,
    Trailer,
    /// A member ended; another may start.
    MemberEnd,
    Done,
}

/// A streaming gzip decoder.
///
/// Multi-member streams (RFC 1952 section 2.2) are decoded as one by
/// default, up to [`Limits::max_members`]; the output cap applies to all
/// members together. With more input possible after a member, `decompress`
/// returns `NeedsInput`; `finish` decides that the stream ended.
///
/// # Examples
///
/// ```
/// use compress_lib::{deflate::Status, gzip};
///
/// let packed = gzip::compress(b"streamed");
/// let mut dec = gzip::Decoder::new();
/// let mut out = [0u8; 32];
/// let p = dec.finish(&packed, &mut out)?;
/// assert_eq!(p.status, Status::Done);
/// assert_eq!(&out[..p.produced], b"streamed");
/// # Ok::<(), compress_lib::Error>(())
/// ```
pub struct Decoder {
    inflater: Inflater,
    limits: Limits,
    state: State,
    multi: bool,
    allow_trailing: bool,
    keep_header: bool,
    header: Option<GzipHeader>,
    /// Header bytes and fields in progress.
    fixed: [u8; 10],
    index: usize,
    extra_left: usize,
    header_crc: Crc32,
    header_bytes: usize,
    field: Vec<u8>,
    crc: Crc32,
    member_len: u32,
    trailer: [u8; 8],
    members: u32,
    out_before: u64,
    consumed_total: u64,
    failed: Option<Error>,
}

impl core::fmt::Debug for Decoder {
    /// Header fields and checksums in progress are not shown.
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Decoder")
            .field("inflater", &self.inflater)
            .field("limits", &self.limits)
            .field("state", &self.state)
            .field("multi", &self.multi)
            .field("allow_trailing", &self.allow_trailing)
            .field("members", &self.members)
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
    /// A decoder with window bits 15 and the default limits, multi-member,
    /// trailing data refused, header fields not kept.
    ///
    /// # Examples
    ///
    /// ```
    /// assert!(!compress_lib::gzip::Decoder::new().is_done());
    /// ```
    #[must_use]
    pub fn new() -> Self {
        Self::from_inflater(Inflater::new(), Limits::new())
    }

    pub(crate) fn one_shot(limits: Limits) -> Self {
        Self::from_inflater(Inflater::without_history(limits), limits)
    }

    fn from_inflater(inflater: Inflater, limits: Limits) -> Self {
        Self {
            inflater,
            limits,
            state: State::Fixed,
            multi: true,
            allow_trailing: false,
            keep_header: false,
            header: None,
            fixed: [0; 10],
            index: 0,
            extra_left: 0,
            header_crc: Crc32::new(),
            header_bytes: 0,
            field: Vec::new(),
            crc: Crc32::new(),
            member_len: 0,
            trailer: [0; 8],
            members: 0,
            out_before: 0,
            consumed_total: 0,
            failed: None,
        }
    }

    /// Sets the limits: the output cap (across members), the ratio cap, the
    /// header field cap, and the member cap.
    ///
    /// # Examples
    ///
    /// ```
    /// use compress_lib::{Limits, gzip};
    ///
    /// let dec = gzip::Decoder::new().with_limits(Limits::new().with_max_members(1));
    /// # let _ = dec;
    /// ```
    #[must_use]
    pub fn with_limits(mut self, limits: Limits) -> Self {
        self.inflater = self.inflater.with_limits(limits);
        self.limits = limits;
        self
    }

    /// Decodes one member only; `Done` at its end, and what follows is left
    /// to the caller.
    ///
    /// # Examples
    ///
    /// ```
    /// use compress_lib::{deflate::Status, gzip};
    ///
    /// let mut two = gzip::compress(b"first");
    /// let first_len = two.len();
    /// two.extend_from_slice(&gzip::compress(b"second"));
    /// let mut dec = gzip::Decoder::new().with_single_member();
    /// let mut out = Vec::new();
    /// let p = dec.decompress_vec(&two, &mut out)?;
    /// assert_eq!((p.status, p.consumed), (Status::Done, first_len));
    /// # Ok::<(), compress_lib::Error>(())
    /// ```
    #[must_use]
    pub fn with_single_member(mut self) -> Self {
        self.multi = false;
        self
    }

    /// Allows bytes that are not a gzip member after the last member (some
    /// tools pad files with zeros); `consumed` stops before them.
    ///
    /// # Examples
    ///
    /// ```
    /// use compress_lib::gzip;
    ///
    /// let mut padded = gzip::compress(b"data");
    /// padded.extend_from_slice(&[0; 8]);
    /// let mut dec = gzip::Decoder::new().with_trailing_data(true);
    /// let mut out = Vec::new();
    /// dec.finish_vec(&padded, &mut out)?;
    /// assert_eq!(out, b"data");
    /// # Ok::<(), compress_lib::Error>(())
    /// ```
    #[must_use]
    pub fn with_trailing_data(mut self, allow: bool) -> Self {
        self.allow_trailing = allow;
        self
    }

    /// Keeps the header fields (name, comment, extra) for [`header`]
    /// (bounded by [`Limits::max_header`]); off by default, so no memory is
    /// spent on them.
    ///
    /// [`header`]: Decoder::header
    ///
    /// # Examples
    ///
    /// ```
    /// let dec = compress_lib::gzip::Decoder::new().with_keep_header(true);
    /// assert!(dec.header().is_none());
    /// ```
    #[must_use]
    pub fn with_keep_header(mut self, keep: bool) -> Self {
        self.keep_header = keep;
        self
    }

    /// The first member's header, once read, when headers are kept.
    ///
    /// # Examples
    ///
    /// ```
    /// use compress_lib::gzip;
    ///
    /// let mut dec = gzip::Decoder::new().with_keep_header(true);
    /// dec.finish_vec(&gzip::compress(b"x"), &mut Vec::new())?;
    /// assert_eq!(dec.header().map(|h| h.os()), Some(255));
    /// # Ok::<(), compress_lib::Error>(())
    /// ```
    #[must_use]
    pub fn header(&self) -> Option<&GzipHeader> {
        self.header.as_ref()
    }

    /// `true` once the stream ended.
    ///
    /// # Examples
    ///
    /// ```
    /// use compress_lib::gzip;
    ///
    /// let mut dec = gzip::Decoder::new();
    /// dec.finish_vec(&gzip::compress(b""), &mut Vec::new())?;
    /// assert!(dec.is_done());
    /// # Ok::<(), compress_lib::Error>(())
    /// ```
    #[must_use]
    pub fn is_done(&self) -> bool {
        self.state == State::Done
    }

    /// Members completed so far.
    ///
    /// # Examples
    ///
    /// ```
    /// use compress_lib::gzip;
    ///
    /// let mut two = gzip::compress(b"a");
    /// two.extend_from_slice(&gzip::compress(b"b"));
    /// let mut dec = gzip::Decoder::new();
    /// dec.finish_vec(&two, &mut Vec::new())?;
    /// assert_eq!(dec.members(), 2);
    /// # Ok::<(), compress_lib::Error>(())
    /// ```
    #[must_use]
    pub fn members(&self) -> u32 {
        self.members
    }

    /// Compressed bytes consumed.
    ///
    /// # Examples
    ///
    /// ```
    /// use compress_lib::gzip;
    ///
    /// let packed = gzip::compress(b"x");
    /// let mut dec = gzip::Decoder::new();
    /// dec.finish_vec(&packed, &mut Vec::new())?;
    /// assert_eq!(dec.total_in(), packed.len() as u64);
    /// # Ok::<(), compress_lib::Error>(())
    /// ```
    #[must_use]
    pub fn total_in(&self) -> u64 {
        self.consumed_total
    }

    /// Decompressed bytes produced, all members.
    ///
    /// # Examples
    ///
    /// ```
    /// use compress_lib::gzip;
    ///
    /// let mut dec = gzip::Decoder::new();
    /// dec.finish_vec(&gzip::compress(b"four"), &mut Vec::new())?;
    /// assert_eq!(dec.total_out(), 4);
    /// # Ok::<(), compress_lib::Error>(())
    /// ```
    #[must_use]
    pub fn total_out(&self) -> u64 {
        self.out_before + self.inflater.total_out()
    }

    /// Heap bytes held.
    ///
    /// # Examples
    ///
    /// ```
    /// assert!(compress_lib::gzip::Decoder::new().memory_in_use() > 32 * 1024);
    /// ```
    #[must_use]
    pub fn memory_in_use(&self) -> usize {
        self.inflater.memory_in_use() + self.field.capacity()
    }

    /// Starts over for a new stream, keeping allocations.
    ///
    /// # Examples
    ///
    /// ```
    /// let mut dec = compress_lib::gzip::Decoder::new();
    /// dec.reset();
    /// ```
    pub fn reset(&mut self) {
        self.inflater.reset();
        self.inflater.set_max_output(self.limits.max_output());
        self.state = State::Fixed;
        self.index = 0;
        self.header = None;
        self.field.clear();
        self.crc = Crc32::new();
        self.header_crc = Crc32::new();
        self.header_bytes = 0;
        self.extra_left = 0;
        self.member_len = 0;
        self.trailer = [0; 8];
        self.members = 0;
        self.out_before = 0;
        self.consumed_total = 0;
        self.failed = None;
    }

    /// Zeroes the window, the decode tables, and the header fields read so
    /// far, then resets the decoder. For connections that carried secrets.
    ///
    /// The zeroing goes through [`core::hint::black_box`]; see
    /// [`Inflater::wipe`](crate::deflate::Inflater::wipe) for what that can
    /// promise without `unsafe`.
    ///
    /// # Examples
    ///
    /// ```
    /// use compress_lib::gzip;
    ///
    /// let mut dec = gzip::Decoder::new();
    /// dec.finish_vec(&gzip::compress(b"token"), &mut Vec::new())?;
    /// dec.wipe();
    /// assert_eq!(dec.total_out(), 0);
    /// # Ok::<(), compress_lib::Error>(())
    /// ```
    pub fn wipe(&mut self) {
        self.inflater.wipe();
        crate::wipe_vec(&mut self.field);
        if let Some(header) = &mut self.header {
            header.wipe();
        }
        self.fixed = [0; 10];
        self.trailer = [0; 8];
        core::hint::black_box((&self.fixed, &self.trailer));
        self.reset();
    }

    /// Decompresses as much of `input` into `output` as fits; more input may
    /// follow (and after a member, perhaps another member).
    ///
    /// # Errors
    ///
    /// Decoding errors, the header and trailer checks, and the limits.
    ///
    /// # Examples
    ///
    /// ```
    /// use compress_lib::{deflate::Status, gzip};
    ///
    /// let packed = gzip::compress(b"split");
    /// let mut dec = gzip::Decoder::new();
    /// let mut out = [0u8; 16];
    /// let a = dec.decompress(&packed[..5], &mut out)?;
    /// assert_eq!(a.status, Status::NeedsInput);
    /// let b = dec.finish(&packed[5..], &mut out[a.produced..])?;
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
    /// use compress_lib::{ErrorKind, gzip};
    ///
    /// let packed = gzip::compress(b"data");
    /// let err = gzip::Decoder::new().finish(&packed[..packed.len() - 1], &mut [0u8; 8]).unwrap_err();
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
    /// use compress_lib::gzip;
    ///
    /// let mut out = Vec::new();
    /// gzip::Decoder::new().decompress_vec(&gzip::compress(b"vec"), &mut out)?;
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
    /// use compress_lib::gzip;
    ///
    /// let mut out = Vec::new();
    /// gzip::Decoder::new().finish_vec(&gzip::compress(b"vec"), &mut out)?;
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

    /// Counts one byte of an optional header field against the cap.
    fn header_byte(&mut self, b: u8) -> Result<(), ErrorKind> {
        self.header_crc.update(&[b]);
        self.header_bytes += 1;
        if self.header_bytes > self.limits.max_header() {
            return Err(ErrorKind::HeaderTooLarge);
        }
        if self.keep_header && self.members == 0 {
            if self.field.try_reserve(1).is_err() {
                return Err(ErrorKind::AllocationFailed);
            }
            self.field.push(b);
        }
        Ok(())
    }

    fn keep_field(&mut self, which: State) {
        if !(self.keep_header && self.members == 0) {
            return;
        }
        let value = core::mem::take(&mut self.field);
        if let Some(h) = &mut self.header {
            match which {
                State::Extra => h.extra = Some(value),
                State::Name => h.name = Some(value),
                _ => h.comment = Some(value),
            }
        }
    }

    /// The header state after `from`, by the flags.
    fn next_header_state(&self, from: State) -> State {
        let flg = self.fixed[3];
        let order = [
            (State::ExtraLength, FEXTRA),
            (State::Name, FNAME),
            (State::Comment, FCOMMENT),
            (State::HeaderCrc, FHCRC),
        ];
        let start = match from {
            State::Fixed => 0,
            State::Extra | State::ExtraLength => 1,
            State::Name => 2,
            State::Comment => 3,
            _ => 4,
        };
        for &(state, bit) in &order[start..] {
            if flg & bit != 0 {
                return state;
            }
        }
        State::Body
    }

    fn run(&mut self, input: &[u8], mut sink: Sink<'_>, last: bool) -> Result<Progress, Error> {
        if let Some(e) = self.failed {
            return Err(e);
        }
        let mut ip = 0;
        let mut produced = 0;
        let status = loop {
            match self.state {
                State::Fixed => {
                    while self.index < 10 && ip < input.len() {
                        self.fixed[self.index] = input[ip];
                        self.index += 1;
                        ip += 1;
                        // Reject a wrong magic number at once (also for the
                        // trailing-data check after a member).
                        if self.index == 2 && self.fixed[..2] != [ID1, ID2] {
                            return Err(self.fail(ErrorKind::InvalidHeader, ip));
                        }
                    }
                    if self.index < 10 {
                        break Status::NeedsInput;
                    }
                    let f = self.fixed;
                    if f[2] != 8 || f[3] & 0xE0 != 0 {
                        return Err(self.fail(ErrorKind::InvalidHeader, ip));
                    }
                    self.header_crc = Crc32::new();
                    self.header_crc.update(&f);
                    self.header_bytes = 0;
                    if self.keep_header && self.members == 0 {
                        self.header = Some(GzipHeader {
                            mtime: u32::from_le_bytes([f[4], f[5], f[6], f[7]]),
                            extra_flags: f[8],
                            os: f[9],
                            text: f[3] & FTEXT != 0,
                            name: None,
                            comment: None,
                            extra: None,
                        });
                    }
                    self.index = 0;
                    self.state = self.next_header_state(State::Fixed);
                }
                State::ExtraLength => {
                    while self.index < 2 && ip < input.len() {
                        self.trailer[self.index] = input[ip];
                        self.index += 1;
                        ip += 1;
                    }
                    if self.index < 2 {
                        break Status::NeedsInput;
                    }
                    let xlen = [self.trailer[0], self.trailer[1]];
                    self.header_crc.update(&xlen);
                    self.extra_left = usize::from(u16::from_le_bytes(xlen));
                    self.index = 0;
                    self.state = State::Extra;
                }
                State::Extra => {
                    while self.extra_left > 0 && ip < input.len() {
                        let b = input[ip];
                        ip += 1;
                        self.extra_left -= 1;
                        if let Err(kind) = self.header_byte(b) {
                            return Err(self.fail(kind, ip));
                        }
                    }
                    if self.extra_left > 0 {
                        break Status::NeedsInput;
                    }
                    self.keep_field(State::Extra);
                    self.state = self.next_header_state(State::Extra);
                }
                State::Name | State::Comment => {
                    let which = self.state;
                    let mut ended = false;
                    while ip < input.len() {
                        let b = input[ip];
                        ip += 1;
                        if b == 0 {
                            self.header_crc.update(&[0]);
                            ended = true;
                            break;
                        }
                        if let Err(kind) = self.header_byte(b) {
                            return Err(self.fail(kind, ip));
                        }
                    }
                    if !ended {
                        break Status::NeedsInput;
                    }
                    self.keep_field(which);
                    self.state = self.next_header_state(which);
                }
                State::HeaderCrc => {
                    while self.index < 2 && ip < input.len() {
                        self.trailer[self.index] = input[ip];
                        self.index += 1;
                        ip += 1;
                    }
                    if self.index < 2 {
                        break Status::NeedsInput;
                    }
                    self.index = 0;
                    let want = u16::from_le_bytes([self.trailer[0], self.trailer[1]]);
                    if want != (self.header_crc.finish() & 0xFFFF) as u16 {
                        return Err(self.fail(ErrorKind::ChecksumMismatch, ip));
                    }
                    self.state = State::Body;
                }
                State::Body => {
                    let r = match &mut sink {
                        Sink::Slice(out) => self
                            .inflater
                            .decompress(&input[ip..], &mut out[produced..])
                            .inspect(|p| {
                                self.crc.update(&out[produced..produced + p.produced]);
                            }),
                        Sink::Vec(out) => {
                            let at = out.len();
                            self.inflater
                                .decompress_vec(&input[ip..], out)
                                .inspect(|_| {
                                    self.crc.update(&out[at..]);
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
                    self.member_len = self.member_len.wrapping_add(p.produced as u32);
                    match p.status {
                        Status::Done => {
                            self.index = 0;
                            self.state = State::Trailer;
                        }
                        Status::NeedsOutput => break Status::NeedsOutput,
                        _ => break Status::NeedsInput,
                    }
                }
                State::Trailer => {
                    while self.index < 8 && ip < input.len() {
                        self.trailer[self.index] = input[ip];
                        self.index += 1;
                        ip += 1;
                    }
                    if self.index < 8 {
                        break Status::NeedsInput;
                    }
                    let t = self.trailer;
                    if u32::from_le_bytes([t[0], t[1], t[2], t[3]]) != self.crc.finish() {
                        return Err(self.fail(ErrorKind::ChecksumMismatch, ip));
                    }
                    if u32::from_le_bytes([t[4], t[5], t[6], t[7]]) != self.member_len {
                        return Err(self.fail(ErrorKind::LengthMismatch, ip));
                    }
                    self.members += 1;
                    self.state = if self.multi {
                        State::MemberEnd
                    } else {
                        State::Done
                    };
                }
                State::MemberEnd => {
                    if ip == input.len() {
                        if last {
                            self.state = State::Done;
                            break Status::Done;
                        }
                        break Status::NeedsInput;
                    }
                    if input[ip] != ID1 || (ip + 1 < input.len() && input[ip + 1] != ID2) {
                        if self.allow_trailing {
                            self.state = State::Done;
                            break Status::Done;
                        }
                        return Err(self.fail(ErrorKind::TrailingData, ip));
                    }
                    if self.members >= self.limits.max_members() {
                        return Err(self.fail(ErrorKind::TooManyMembers, ip));
                    }
                    // A new member: a new DEFLATE stream (no history across
                    // members), with the output cap and the ratio bound
                    // shared by all (the ratio counts every byte taken so
                    // far, headers and trailers included).
                    self.out_before += self.inflater.total_out();
                    self.inflater.reset();
                    self.inflater
                        .set_max_output(self.limits.max_output().saturating_sub(self.out_before));
                    self.inflater
                        .set_ratio_base(self.consumed_total + ip as u64, self.out_before);
                    self.crc = Crc32::new();
                    self.member_len = 0;
                    self.index = 0;
                    self.state = State::Fixed;
                }
                State::Done => break Status::Done,
            }
        };
        self.consumed_total += ip as u64;
        if status == Status::NeedsInput && last {
            // An empty stream, or one cut inside a member.
            return Err(self.fail(ErrorKind::UnexpectedEnd, 0));
        }
        Ok(Progress::new(ip, produced, status))
    }
}

/// A streaming gzip encoder (one member).
///
/// # Examples
///
/// ```
/// use compress_lib::{Level, deflate::Flush, gzip};
///
/// let mut enc = gzip::Encoder::new(Level::DEFAULT);
/// let mut packed = Vec::new();
/// enc.compress_vec(b"part one, ", &mut packed, Flush::None)?;
/// enc.compress_vec(b"part two", &mut packed, Flush::Finish)?;
/// assert_eq!(gzip::decompress(&packed)?, b"part one, part two");
/// # Ok::<(), compress_lib::Error>(())
/// ```
pub struct Encoder {
    deflater: Deflater,
    header: GzipHeader,
    crc: Crc32,
    pending: Vec<u8>,
    pending_pos: usize,
    header_written: bool,
    trailer_written: bool,
}

impl core::fmt::Debug for Encoder {
    /// Output in waiting and the header fields are not shown.
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Encoder")
            .field("deflater", &self.deflater)
            .field("pending", &(self.pending.len() - self.pending_pos))
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
    /// An encoder at `level` with the plain header of [`GzipHeader::new`].
    ///
    /// # Examples
    ///
    /// ```
    /// let enc = compress_lib::gzip::Encoder::new(compress_lib::Level::BEST);
    /// # let _ = enc;
    /// ```
    #[must_use]
    pub fn new(level: Level) -> Self {
        Self {
            deflater: Deflater::new(level),
            header: GzipHeader::new(),
            crc: Crc32::new(),
            pending: Vec::new(),
            pending_pos: 0,
            header_written: false,
            trailer_written: false,
        }
    }

    /// Writes `header` instead of the plain one.
    ///
    /// # Examples
    ///
    /// ```
    /// use compress_lib::{Level, gzip::{Encoder, GzipHeader}};
    ///
    /// let enc = Encoder::new(Level::DEFAULT).with_header(GzipHeader::new().with_name(b"a.txt"));
    /// # let _ = enc;
    /// ```
    #[must_use]
    pub fn with_header(mut self, header: GzipHeader) -> Self {
        self.header = header;
        self
    }

    /// Sets the window to `2^bits` bytes, 8 to 15.
    ///
    /// # Errors
    ///
    /// [`ErrorKind::InvalidWindowBits`] outside 8 to 15.
    ///
    /// # Examples
    ///
    /// ```
    /// let enc = compress_lib::gzip::Encoder::default().with_window_bits(12)?;
    /// # let _ = enc;
    /// # Ok::<(), compress_lib::Error>(())
    /// ```
    pub fn with_window_bits(mut self, bits: u8) -> Result<Self, Error> {
        self.deflater = self.deflater.with_window_bits(bits)?;
        Ok(self)
    }

    /// Starts a new member, keeping allocations and the header.
    ///
    /// # Examples
    ///
    /// ```
    /// let mut enc = compress_lib::gzip::Encoder::default();
    /// enc.reset();
    /// ```
    pub fn reset(&mut self) {
        self.deflater.reset();
        self.crc = Crc32::new();
        self.pending.clear();
        self.pending_pos = 0;
        self.header_written = false;
        self.trailer_written = false;
    }

    /// Zeroes the window, hash chains, and buffers (they hold plaintext),
    /// then starts a new member. The header given to the encoder is kept.
    /// For connections that carried secrets.
    ///
    /// The zeroing goes through [`core::hint::black_box`]; see
    /// [`Inflater::wipe`](crate::deflate::Inflater::wipe) for what that can
    /// promise without `unsafe`.
    ///
    /// # Examples
    ///
    /// ```
    /// use compress_lib::{Level, deflate::Flush, gzip};
    ///
    /// let mut enc = gzip::Encoder::new(Level::DEFAULT);
    /// enc.compress_vec(b"token=secret", &mut Vec::new(), Flush::Finish)?;
    /// enc.wipe();
    /// assert_eq!(enc.total_in(), 0);
    /// # Ok::<(), compress_lib::Error>(())
    /// ```
    pub fn wipe(&mut self) {
        self.deflater.wipe();
        crate::wipe_vec(&mut self.pending);
        self.reset();
    }

    /// Uncompressed bytes taken.
    ///
    /// # Examples
    ///
    /// ```
    /// use compress_lib::{deflate::Flush, gzip};
    ///
    /// let mut enc = gzip::Encoder::default();
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
    /// assert!(compress_lib::gzip::Encoder::default().memory_in_use() > 0);
    /// ```
    #[must_use]
    pub fn memory_in_use(&self) -> usize {
        self.deflater.memory_in_use() + self.pending.capacity()
    }

    fn drain(&mut self, output: &mut [u8], produced: &mut usize) -> usize {
        let n = (self.pending.len() - self.pending_pos).min(output.len() - *produced);
        output[*produced..*produced + n]
            .copy_from_slice(&self.pending[self.pending_pos..self.pending_pos + n]);
        self.pending_pos += n;
        *produced += n;
        self.pending.len() - self.pending_pos
    }

    /// As [`Deflater::compress`], with the header written first and the
    /// CRC-32 and length after `Finish`.
    ///
    /// # Errors
    ///
    /// As `Deflater::compress`.
    ///
    /// # Examples
    ///
    /// ```
    /// use compress_lib::{deflate::{Flush, Status}, gzip};
    ///
    /// let mut enc = gzip::Encoder::default();
    /// let mut out = [0u8; 64];
    /// let p = enc.compress(b"abc", &mut out, Flush::Finish)?;
    /// assert_eq!(p.status, Status::Done);
    /// assert_eq!(gzip::decompress(&out[..p.produced])?, b"abc");
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
            let xfl = match self.deflater.level().get() {
                9 => 2,
                1 => 4,
                _ => 0,
            };
            self.pending = self.header.encode(xfl);
            self.pending_pos = 0;
            self.header_written = true;
        }
        if self.drain(output, &mut produced) > 0 {
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
        self.crc.update(&input[..p.consumed]);
        produced += p.produced;
        if p.status == Status::Done && flush == Flush::Finish {
            self.pending.clear();
            self.pending
                .extend_from_slice(&self.crc.finish().to_le_bytes());
            self.pending
                .extend_from_slice(&(self.crc.len() as u32).to_le_bytes());
            self.pending_pos = 0;
            self.trailer_written = true;
            let left = self.drain(output, &mut produced);
            let status = if left == 0 {
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
    /// use compress_lib::{deflate::Flush, gzip};
    ///
    /// let mut out = Vec::new();
    /// gzip::Encoder::default().compress_vec(b"abc", &mut out, Flush::Finish)?;
    /// assert_eq!(gzip::decompress(&out)?, b"abc");
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
