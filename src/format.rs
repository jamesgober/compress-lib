//! One decoder and one encoder type for code that picks the format at run
//! time (an HTTP layer choosing by `Content-Encoding`, for example).

use alloc::vec::Vec;

use crate::{
    Error, ErrorKind, Level, Limits, Progress,
    deflate::{Deflater, Flush, Inflater},
    gzip, zlib,
};

/// A compressed format.
///
/// # Examples
///
/// ```
/// use compress_lib::{Format, gzip, zlib};
///
/// assert_eq!(Format::detect(&gzip::compress(b"x")), Some(Format::Gzip));
/// assert_eq!(Format::detect(&zlib::compress(b"x")), Some(Format::Zlib));
/// assert_eq!(Format::detect(b"plain text"), None);
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Format {
    /// Raw DEFLATE (RFC 1951): no header, no checksum.
    Raw,
    /// zlib (RFC 1950); also HTTP's `deflate` content coding.
    Zlib,
    /// gzip (RFC 1952); HTTP's `gzip` content coding.
    Gzip,
}

impl Format {
    /// Recognises gzip by its magic number and zlib by its header check.
    /// Raw DEFLATE has no header and is never guessed. Needs at least two
    /// bytes.
    ///
    /// # Examples
    ///
    /// ```
    /// use compress_lib::Format;
    ///
    /// assert_eq!(Format::detect(&[0x1F, 0x8B]), Some(Format::Gzip));
    /// assert_eq!(Format::detect(&[0x78, 0x9C]), Some(Format::Zlib));
    /// assert_eq!(Format::detect(&[0x78]), None);
    /// ```
    #[must_use]
    pub fn detect(data: &[u8]) -> Option<Format> {
        match data {
            [0x1F, 0x8B, ..] => Some(Format::Gzip),
            [cmf, flg, ..] if zlib::header_ok(*cmf, *flg) => Some(Format::Zlib),
            _ => None,
        }
    }
}

/// Decompresses a complete gzip or zlib stream, detected from its first
/// bytes, with the default [`Limits`].
///
/// # Errors
///
/// [`ErrorKind::InvalidHeader`] when the data is neither; otherwise as
/// [`gzip::decompress`] or [`zlib::decompress`].
///
/// # Examples
///
/// ```
/// use compress_lib as compress;
///
/// assert_eq!(compress::decompress(&compress::gzip::compress(b"either"))?, b"either");
/// assert_eq!(compress::decompress(&compress::zlib::compress(b"either"))?, b"either");
/// # Ok::<(), compress_lib::Error>(())
/// ```
pub fn decompress(data: &[u8]) -> Result<Vec<u8>, Error> {
    decompress_with(data, Limits::new())
}

/// As [`decompress`], with the given limits.
///
/// # Errors
///
/// As [`decompress`].
///
/// # Examples
///
/// ```
/// use compress_lib as compress;
///
/// let limits = compress::Limits::new().with_max_output(1 << 20);
/// assert_eq!(compress::decompress_with(&compress::gzip::compress(b"x"), limits)?, b"x");
/// # Ok::<(), compress_lib::Error>(())
/// ```
pub fn decompress_with(data: &[u8], limits: Limits) -> Result<Vec<u8>, Error> {
    match Format::detect(data) {
        Some(Format::Gzip) => gzip::decompress_with(data, limits),
        Some(Format::Zlib) => zlib::decompress_with(data, limits),
        _ => Err(Error::new(ErrorKind::InvalidHeader, 0)),
    }
}

#[derive(Debug)]
enum Inner<R, Z, G> {
    Raw(R),
    Zlib(Z),
    Gzip(G),
}

/// A streaming decoder for any [`Format`].
///
/// # Examples
///
/// ```
/// use compress_lib::{Decoder, Format, gzip};
///
/// let mut dec = Decoder::new(Format::Gzip);
/// let mut out = Vec::new();
/// dec.finish_vec(&gzip::compress(b"any format"), &mut out)?;
/// assert_eq!(out, b"any format");
/// # Ok::<(), compress_lib::Error>(())
/// ```
#[derive(Debug)]
pub struct Decoder(Inner<Inflater, zlib::Decoder, gzip::Decoder>);

impl Decoder {
    /// A decoder for `format` with the default limits.
    ///
    /// # Examples
    ///
    /// ```
    /// let dec = compress_lib::Decoder::new(compress_lib::Format::Zlib);
    /// assert_eq!(dec.format(), compress_lib::Format::Zlib);
    /// ```
    #[must_use]
    pub fn new(format: Format) -> Self {
        Self(match format {
            Format::Raw => Inner::Raw(Inflater::new()),
            Format::Zlib => Inner::Zlib(zlib::Decoder::new()),
            Format::Gzip => Inner::Gzip(gzip::Decoder::new()),
        })
    }

    /// Sets the limits checked while decoding.
    ///
    /// # Examples
    ///
    /// ```
    /// use compress_lib::{Decoder, Format, Limits};
    ///
    /// let dec = Decoder::new(Format::Gzip).with_limits(Limits::new().with_max_output(1 << 20));
    /// # let _ = dec;
    /// ```
    #[must_use]
    pub fn with_limits(self, limits: Limits) -> Self {
        Self(match self.0 {
            Inner::Raw(d) => Inner::Raw(d.with_limits(limits)),
            Inner::Zlib(d) => Inner::Zlib(d.with_limits(limits)),
            Inner::Gzip(d) => Inner::Gzip(d.with_limits(limits)),
        })
    }

    /// Lets a gzip decoder stop before bytes that do not start a member
    /// (raw and zlib decoders stop at their end anyway; the caller decides
    /// what follows).
    #[cfg(feature = "std")]
    pub(crate) fn with_trailing_data(self, allow: bool) -> Self {
        Self(match self.0 {
            Inner::Gzip(d) => Inner::Gzip(d.with_trailing_data(allow)),
            other => other,
        })
    }

    /// The format decoded.
    ///
    /// # Examples
    ///
    /// ```
    /// use compress_lib::{Decoder, Format};
    ///
    /// assert_eq!(Decoder::new(Format::Raw).format(), Format::Raw);
    /// ```
    #[must_use]
    pub fn format(&self) -> Format {
        match self.0 {
            Inner::Raw(_) => Format::Raw,
            Inner::Zlib(_) => Format::Zlib,
            Inner::Gzip(_) => Format::Gzip,
        }
    }

    /// As [`Inflater::decompress`].
    ///
    /// # Errors
    ///
    /// The errors of the format's decoder.
    ///
    /// # Examples
    ///
    /// ```
    /// use compress_lib::{Decoder, Format, deflate::Status, zlib};
    ///
    /// let mut out = [0u8; 16];
    /// let p = Decoder::new(Format::Zlib).decompress(&zlib::compress(b"abc"), &mut out)?;
    /// assert_eq!(p.status, Status::Done);
    /// # Ok::<(), compress_lib::Error>(())
    /// ```
    pub fn decompress(&mut self, input: &[u8], output: &mut [u8]) -> Result<Progress, Error> {
        match &mut self.0 {
            Inner::Raw(d) => d.decompress(input, output),
            Inner::Zlib(d) => d.decompress(input, output),
            Inner::Gzip(d) => d.decompress(input, output),
        }
    }

    /// As [`Inflater::finish`].
    ///
    /// # Errors
    ///
    /// The errors of the format's decoder, plus `UnexpectedEnd`.
    ///
    /// # Examples
    ///
    /// ```
    /// use compress_lib::{Decoder, Format, gzip};
    ///
    /// let mut out = [0u8; 16];
    /// let p = Decoder::new(Format::Gzip).finish(&gzip::compress(b"abc"), &mut out)?;
    /// assert_eq!(&out[..p.produced], b"abc");
    /// # Ok::<(), compress_lib::Error>(())
    /// ```
    pub fn finish(&mut self, input: &[u8], output: &mut [u8]) -> Result<Progress, Error> {
        match &mut self.0 {
            Inner::Raw(d) => d.finish(input, output),
            Inner::Zlib(d) => d.finish(input, output),
            Inner::Gzip(d) => d.finish(input, output),
        }
    }

    /// As [`Inflater::decompress_vec`].
    ///
    /// # Errors
    ///
    /// The errors of the format's decoder, plus `AllocationFailed`.
    ///
    /// # Examples
    ///
    /// ```
    /// use compress_lib::{Decoder, Format, deflate};
    ///
    /// let mut out = Vec::new();
    /// Decoder::new(Format::Raw).decompress_vec(&deflate::compress(b"raw"), &mut out)?;
    /// assert_eq!(out, b"raw");
    /// # Ok::<(), compress_lib::Error>(())
    /// ```
    pub fn decompress_vec(&mut self, input: &[u8], out: &mut Vec<u8>) -> Result<Progress, Error> {
        match &mut self.0 {
            Inner::Raw(d) => d.decompress_vec(input, out),
            Inner::Zlib(d) => d.decompress_vec(input, out),
            Inner::Gzip(d) => d.decompress_vec(input, out),
        }
    }

    /// As [`Inflater::finish_vec`].
    ///
    /// # Errors
    ///
    /// The errors of the format's decoder, plus `UnexpectedEnd` and
    /// `AllocationFailed`.
    ///
    /// # Examples
    ///
    /// ```
    /// use compress_lib::{Decoder, Format, zlib};
    ///
    /// let mut out = Vec::new();
    /// Decoder::new(Format::Zlib).finish_vec(&zlib::compress(b"z"), &mut out)?;
    /// assert_eq!(out, b"z");
    /// # Ok::<(), compress_lib::Error>(())
    /// ```
    pub fn finish_vec(&mut self, input: &[u8], out: &mut Vec<u8>) -> Result<Progress, Error> {
        match &mut self.0 {
            Inner::Raw(d) => d.finish_vec(input, out),
            Inner::Zlib(d) => d.finish_vec(input, out),
            Inner::Gzip(d) => d.finish_vec(input, out),
        }
    }

    /// `true` once the stream ended.
    ///
    /// # Examples
    ///
    /// ```
    /// assert!(!compress_lib::Decoder::new(compress_lib::Format::Gzip).is_done());
    /// ```
    #[must_use]
    pub fn is_done(&self) -> bool {
        match &self.0 {
            Inner::Raw(d) => d.is_done(),
            Inner::Zlib(d) => d.is_done(),
            Inner::Gzip(d) => d.is_done(),
        }
    }

    /// Compressed bytes consumed.
    ///
    /// # Examples
    ///
    /// ```
    /// assert_eq!(compress_lib::Decoder::new(compress_lib::Format::Gzip).total_in(), 0);
    /// ```
    #[must_use]
    pub fn total_in(&self) -> u64 {
        match &self.0 {
            Inner::Raw(d) => d.total_in(),
            Inner::Zlib(d) => d.total_in(),
            Inner::Gzip(d) => d.total_in(),
        }
    }

    /// Decompressed bytes produced.
    ///
    /// # Examples
    ///
    /// ```
    /// assert_eq!(compress_lib::Decoder::new(compress_lib::Format::Gzip).total_out(), 0);
    /// ```
    #[must_use]
    pub fn total_out(&self) -> u64 {
        match &self.0 {
            Inner::Raw(d) => d.total_out(),
            Inner::Zlib(d) => d.total_out(),
            Inner::Gzip(d) => d.total_out(),
        }
    }

    /// Starts over for a new stream, keeping allocations.
    ///
    /// # Examples
    ///
    /// ```
    /// let mut dec = compress_lib::Decoder::new(compress_lib::Format::Zlib);
    /// dec.reset();
    /// ```
    pub fn reset(&mut self) {
        match &mut self.0 {
            Inner::Raw(d) => d.reset(),
            Inner::Zlib(d) => d.reset(),
            Inner::Gzip(d) => d.reset(),
        }
    }

    /// Zeroes the window, tables, and buffers of the underlying decoder,
    /// then resets it. For connections that carried secrets; see
    /// [`Inflater::wipe`].
    ///
    /// # Examples
    ///
    /// ```
    /// use compress_lib::{Decoder, Format, gzip};
    ///
    /// let mut dec = Decoder::new(Format::Gzip);
    /// dec.finish_vec(&gzip::compress(b"token"), &mut Vec::new())?;
    /// dec.wipe();
    /// assert_eq!(dec.total_out(), 0);
    /// # Ok::<(), compress_lib::Error>(())
    /// ```
    pub fn wipe(&mut self) {
        match &mut self.0 {
            Inner::Raw(d) => d.wipe(),
            Inner::Zlib(d) => d.wipe(),
            Inner::Gzip(d) => d.wipe(),
        }
    }
}

/// A streaming encoder for any [`Format`].
///
/// # Examples
///
/// ```
/// use compress_lib::{Encoder, Format, Level, deflate::Flush, gzip};
///
/// let mut enc = Encoder::new(Format::Gzip, Level::DEFAULT);
/// let mut out = Vec::new();
/// enc.compress_vec(b"response body", &mut out, Flush::Finish)?;
/// assert_eq!(gzip::decompress(&out)?, b"response body");
/// # Ok::<(), compress_lib::Error>(())
/// ```
#[derive(Debug)]
pub struct Encoder(Inner<Deflater, zlib::Encoder, gzip::Encoder>);

impl Encoder {
    /// An encoder for `format` at `level`.
    ///
    /// # Examples
    ///
    /// ```
    /// use compress_lib::{Encoder, Format, Level};
    ///
    /// assert_eq!(Encoder::new(Format::Zlib, Level::FAST).format(), Format::Zlib);
    /// ```
    #[must_use]
    pub fn new(format: Format, level: Level) -> Self {
        Self(match format {
            Format::Raw => Inner::Raw(Deflater::new(level)),
            Format::Zlib => Inner::Zlib(zlib::Encoder::new(level)),
            Format::Gzip => Inner::Gzip(gzip::Encoder::new(level)),
        })
    }

    /// The format written.
    ///
    /// # Examples
    ///
    /// ```
    /// use compress_lib::{Encoder, Format, Level};
    ///
    /// assert_eq!(Encoder::new(Format::Raw, Level::FAST).format(), Format::Raw);
    /// ```
    #[must_use]
    pub fn format(&self) -> Format {
        match self.0 {
            Inner::Raw(_) => Format::Raw,
            Inner::Zlib(_) => Format::Zlib,
            Inner::Gzip(_) => Format::Gzip,
        }
    }

    /// As [`Deflater::compress`], with the format's header and trailer.
    ///
    /// # Errors
    ///
    /// As `Deflater::compress`.
    ///
    /// # Examples
    ///
    /// ```
    /// use compress_lib::{Encoder, Format, Level, deflate::{Flush, Status}};
    ///
    /// let mut enc = Encoder::new(Format::Zlib, Level::DEFAULT);
    /// let mut out = [0u8; 64];
    /// assert_eq!(enc.compress(b"abc", &mut out, Flush::Finish)?.status, Status::Done);
    /// # Ok::<(), compress_lib::Error>(())
    /// ```
    pub fn compress(
        &mut self,
        input: &[u8],
        output: &mut [u8],
        flush: Flush,
    ) -> Result<Progress, Error> {
        match &mut self.0 {
            Inner::Raw(e) => e.compress(input, output, flush),
            Inner::Zlib(e) => e.compress(input, output, flush),
            Inner::Gzip(e) => e.compress(input, output, flush),
        }
    }

    /// As [`Deflater::compress_vec`].
    ///
    /// # Errors
    ///
    /// As `compress`, plus `AllocationFailed`.
    ///
    /// # Examples
    ///
    /// ```
    /// use compress_lib::{Encoder, Format, Level, deflate::Flush, zlib};
    ///
    /// let mut out = Vec::new();
    /// Encoder::new(Format::Zlib, Level::DEFAULT).compress_vec(b"abc", &mut out, Flush::Finish)?;
    /// assert_eq!(zlib::decompress(&out)?, b"abc");
    /// # Ok::<(), compress_lib::Error>(())
    /// ```
    pub fn compress_vec(
        &mut self,
        input: &[u8],
        out: &mut Vec<u8>,
        flush: Flush,
    ) -> Result<Progress, Error> {
        match &mut self.0 {
            Inner::Raw(e) => e.compress_vec(input, out, flush),
            Inner::Zlib(e) => e.compress_vec(input, out, flush),
            Inner::Gzip(e) => e.compress_vec(input, out, flush),
        }
    }

    /// Starts a new stream, keeping allocations.
    ///
    /// # Examples
    ///
    /// ```
    /// let mut enc = compress_lib::Encoder::new(compress_lib::Format::Gzip, compress_lib::Level::FAST);
    /// enc.reset();
    /// ```
    pub fn reset(&mut self) {
        match &mut self.0 {
            Inner::Raw(e) => e.reset(),
            Inner::Zlib(e) => e.reset(),
            Inner::Gzip(e) => e.reset(),
        }
    }

    /// Zeroes the window, hash chains, and buffers of the underlying
    /// encoder, then resets it. For connections that carried secrets; see
    /// [`Deflater::wipe`].
    ///
    /// # Examples
    ///
    /// ```
    /// use compress_lib::{Encoder, Format, Level, deflate::Flush};
    ///
    /// let mut enc = Encoder::new(Format::Zlib, Level::DEFAULT);
    /// enc.compress_vec(b"token=secret", &mut Vec::new(), Flush::Finish)?;
    /// enc.wipe();
    /// # Ok::<(), compress_lib::Error>(())
    /// ```
    pub fn wipe(&mut self) {
        match &mut self.0 {
            Inner::Raw(e) => e.wipe(),
            Inner::Zlib(e) => e.wipe(),
            Inner::Gzip(e) => e.wipe(),
        }
    }
}
