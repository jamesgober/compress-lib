//! `std::io` adapters over the streaming engines (feature `std`).
//!
//! | Type | Reads or writes | Does |
//! |---|---|---|
//! | [`Reader`] | reads from a compressed `Read` | decompresses |
//! | [`Writer`] | writes to a `Write` | compresses |
//! | [`Encoder`] | reads from a plain `Read` | compresses |
//! | [`DecodeWriter`] | writes to a `Write` | decompresses |
//!
//! Each holds one 32 KiB buffer besides its engine. Decoders take
//! [`Limits`]; errors come out as `std::io::Error` wrapping the crate's
//! [`Error`](crate::Error).
//!
//! ```
//! use std::io::{Read, Write};
//!
//! use compress_lib::{Format, Level, io};
//!
//! let mut w = io::Writer::new(Vec::new(), Format::Gzip, Level::DEFAULT);
//! w.write_all(b"through std::io")?;
//! let packed = w.finish()?;
//!
//! let mut r = io::Reader::new(&packed[..], Format::Gzip);
//! let mut text = String::new();
//! r.read_to_string(&mut text)?;
//! assert_eq!(text, "through std::io");
//! # Ok::<(), std::io::Error>(())
//! ```

use std::io::{self, Read, Write};

use crate::{Decoder, Encoder as FormatEncoder, Format, Level, Limits, Status, deflate::Flush};

const BUF: usize = 32 * 1024;

/// Decompresses while reading from a compressed source.
///
/// Reading past the end of the compressed stream returns 0. Data after the
/// stream is left unread in the source as far as the 32 KiB buffer allows
/// (use [`Reader::into_inner`] with care), and the stream must be complete:
/// a source that ends early is `UnexpectedEof`.
///
/// # Examples
///
/// ```
/// use std::io::Read;
///
/// use compress_lib::{Format, io::Reader, zlib};
///
/// let packed = zlib::compress(b"read me");
/// let mut out = Vec::new();
/// Reader::new(&packed[..], Format::Zlib).read_to_end(&mut out)?;
/// assert_eq!(out, b"read me");
/// # Ok::<(), std::io::Error>(())
/// ```
#[derive(Debug)]
pub struct Reader<R> {
    inner: R,
    decoder: Decoder,
    buf: Box<[u8]>,
    pos: usize,
    len: usize,
    eof: bool,
}

impl<R: Read> Reader<R> {
    /// Reads `format` from `inner` with the default limits.
    ///
    /// # Examples
    ///
    /// ```
    /// let r = compress_lib::io::Reader::new(&b""[..], compress_lib::Format::Gzip);
    /// # let _ = r;
    /// ```
    pub fn new(inner: R, format: Format) -> Self {
        Self {
            inner,
            decoder: Decoder::new(format),
            buf: vec![0; BUF].into_boxed_slice(),
            pos: 0,
            len: 0,
            eof: false,
        }
    }

    /// Sets the limits checked while decoding.
    ///
    /// # Examples
    ///
    /// ```
    /// use compress_lib::{Format, Limits, io::Reader};
    ///
    /// let r = Reader::new(&b""[..], Format::Gzip).with_limits(Limits::new().with_max_output(1 << 20));
    /// # let _ = r;
    /// ```
    #[must_use]
    pub fn with_limits(mut self, limits: Limits) -> Self {
        self.decoder = self.decoder.with_limits(limits);
        self
    }

    /// The source, consuming the reader.
    ///
    /// # Examples
    ///
    /// ```
    /// let r = compress_lib::io::Reader::new(&b"xyz"[..], compress_lib::Format::Gzip);
    /// let _source: &[u8] = r.into_inner();
    /// ```
    pub fn into_inner(self) -> R {
        self.inner
    }
}

impl<R: Read> Read for Reader<R> {
    fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
        if out.is_empty() || self.decoder.is_done() {
            return Ok(0);
        }
        loop {
            if self.pos == self.len && !self.eof {
                self.len = self.inner.read(&mut self.buf)?;
                self.pos = 0;
                self.eof = self.len == 0;
            }
            let input = &self.buf[self.pos..self.len];
            let p = if self.eof {
                self.decoder.finish(input, out)?
            } else {
                self.decoder.decompress(input, out)?
            };
            self.pos += p.consumed;
            if p.produced > 0 || p.status == Status::Done {
                return Ok(p.produced);
            }
        }
    }
}

/// Compresses while writing to a destination.
///
/// `flush` writes a sync point (everything so far becomes decodable) and
/// flushes the destination; [`finish`](Writer::finish) ends the stream and
/// returns the destination. Dropping a writer without `finish` tries to end
/// the stream and ignores errors, as a drop cannot report them: call
/// `finish`.
///
/// # Examples
///
/// ```
/// use std::io::Write;
///
/// use compress_lib::{Format, Level, deflate, io::Writer};
///
/// let mut w = Writer::new(Vec::new(), Format::Raw, Level::FAST);
/// w.write_all(b"written")?;
/// assert_eq!(deflate::decompress(&w.finish()?).unwrap(), b"written");
/// # Ok::<(), std::io::Error>(())
/// ```
#[derive(Debug)]
pub struct Writer<W: Write> {
    inner: Option<W>,
    encoder: FormatEncoder,
    buf: Box<[u8]>,
    finished: bool,
}

impl<W: Write> Writer<W> {
    /// Writes `format` at `level` to `inner`.
    ///
    /// # Examples
    ///
    /// ```
    /// let w = compress_lib::io::Writer::new(Vec::new(), compress_lib::Format::Gzip, compress_lib::Level::FAST);
    /// # let _ = w;
    /// ```
    pub fn new(inner: W, format: Format, level: Level) -> Self {
        Self {
            inner: Some(inner),
            encoder: FormatEncoder::new(format, level),
            buf: vec![0; BUF].into_boxed_slice(),
            finished: false,
        }
    }

    fn pump(&mut self, mut input: &[u8], flush: Flush) -> io::Result<usize> {
        let mut taken = 0;
        let Some(inner) = self.inner.as_mut() else {
            return Err(io::Error::other("writer already finished"));
        };
        loop {
            let p = self.encoder.compress(input, &mut self.buf, flush)?;
            inner.write_all(&self.buf[..p.produced])?;
            input = &input[p.consumed..];
            taken += p.consumed;
            if p.status != Status::NeedsOutput {
                return Ok(taken);
            }
        }
    }

    /// Ends the stream and returns the destination.
    ///
    /// # Errors
    ///
    /// Errors of the destination.
    ///
    /// # Examples
    ///
    /// ```
    /// use compress_lib::{Format, Level, io::Writer};
    ///
    /// let packed = Writer::new(Vec::new(), Format::Zlib, Level::DEFAULT).finish()?;
    /// assert_eq!(compress_lib::zlib::decompress(&packed).unwrap(), b"");
    /// # Ok::<(), std::io::Error>(())
    /// ```
    pub fn finish(mut self) -> io::Result<W> {
        self.pump(&[], Flush::Finish)?;
        self.finished = true;
        let mut inner = self
            .inner
            .take()
            .ok_or_else(|| io::Error::other("writer already finished"))?;
        inner.flush()?;
        Ok(inner)
    }
}

impl<W: Write> Write for Writer<W> {
    fn write(&mut self, data: &[u8]) -> io::Result<usize> {
        self.pump(data, Flush::None)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.pump(&[], Flush::Sync)?;
        match self.inner.as_mut() {
            Some(inner) => inner.flush(),
            None => Ok(()),
        }
    }
}

impl<W: Write> Drop for Writer<W> {
    fn drop(&mut self) {
        if !self.finished && self.inner.is_some() {
            let _ = self.pump(&[], Flush::Finish);
        }
    }
}

/// Compresses while reading: reads plain data from `inner` and returns it
/// compressed.
///
/// # Examples
///
/// ```
/// use std::io::Read;
///
/// use compress_lib::{Format, Level, gzip, io::Encoder};
///
/// let mut packed = Vec::new();
/// Encoder::new(&b"plain source"[..], Format::Gzip, Level::DEFAULT).read_to_end(&mut packed)?;
/// assert_eq!(gzip::decompress(&packed).unwrap(), b"plain source");
/// # Ok::<(), std::io::Error>(())
/// ```
#[derive(Debug)]
pub struct Encoder<R> {
    inner: R,
    encoder: FormatEncoder,
    buf: Box<[u8]>,
    pos: usize,
    len: usize,
    eof: bool,
    done: bool,
}

impl<R: Read> Encoder<R> {
    /// Compresses `inner` as `format` at `level`.
    ///
    /// # Examples
    ///
    /// ```
    /// let e = compress_lib::io::Encoder::new(&b""[..], compress_lib::Format::Zlib, compress_lib::Level::FAST);
    /// # let _ = e;
    /// ```
    pub fn new(inner: R, format: Format, level: Level) -> Self {
        Self {
            inner,
            encoder: FormatEncoder::new(format, level),
            buf: vec![0; BUF].into_boxed_slice(),
            pos: 0,
            len: 0,
            eof: false,
            done: false,
        }
    }
}

impl<R: Read> Read for Encoder<R> {
    fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
        if out.is_empty() || self.done {
            return Ok(0);
        }
        loop {
            if self.pos == self.len && !self.eof {
                self.len = self.inner.read(&mut self.buf)?;
                self.pos = 0;
                self.eof = self.len == 0;
            }
            let flush = if self.eof { Flush::Finish } else { Flush::None };
            let p = self
                .encoder
                .compress(&self.buf[self.pos..self.len], out, flush)?;
            self.pos += p.consumed;
            if p.status == Status::Done && self.eof {
                self.done = true;
            }
            if p.produced > 0 || self.done {
                return Ok(p.produced);
            }
        }
    }
}

/// Decompresses while writing: compressed data written in comes out plain
/// in `inner`.
///
/// Bytes after the end of the stream are refused (`InvalidData`).
/// [`finish`](DecodeWriter::finish) checks that the stream was complete.
///
/// # Examples
///
/// ```
/// use std::io::Write;
///
/// use compress_lib::{Format, gzip, io::DecodeWriter};
///
/// let mut w = DecodeWriter::new(Vec::new(), Format::Gzip);
/// w.write_all(&gzip::compress(b"decoded on write"))?;
/// assert_eq!(w.finish()?, b"decoded on write");
/// # Ok::<(), std::io::Error>(())
/// ```
#[derive(Debug)]
pub struct DecodeWriter<W: Write> {
    inner: W,
    decoder: Decoder,
    buf: Box<[u8]>,
}

impl<W: Write> DecodeWriter<W> {
    /// Decodes `format` into `inner` with the default limits.
    ///
    /// # Examples
    ///
    /// ```
    /// let w = compress_lib::io::DecodeWriter::new(Vec::new(), compress_lib::Format::Zlib);
    /// # let _ = w;
    /// ```
    pub fn new(inner: W, format: Format) -> Self {
        Self {
            inner,
            decoder: Decoder::new(format),
            buf: vec![0; BUF].into_boxed_slice(),
        }
    }

    /// Sets the limits checked while decoding.
    ///
    /// # Examples
    ///
    /// ```
    /// use compress_lib::{Format, Limits, io::DecodeWriter};
    ///
    /// let w = DecodeWriter::new(Vec::new(), Format::Zlib).with_limits(Limits::new().with_max_output(1 << 20));
    /// # let _ = w;
    /// ```
    #[must_use]
    pub fn with_limits(mut self, limits: Limits) -> Self {
        self.decoder = self.decoder.with_limits(limits);
        self
    }

    fn pump(&mut self, mut data: &[u8], last: bool) -> io::Result<usize> {
        let mut taken = 0;
        loop {
            let p = if last {
                self.decoder.finish(data, &mut self.buf)?
            } else {
                self.decoder.decompress(data, &mut self.buf)?
            };
            self.inner.write_all(&self.buf[..p.produced])?;
            data = &data[p.consumed..];
            taken += p.consumed;
            match p.status {
                Status::NeedsOutput => continue,
                Status::Done if !data.is_empty() => {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        crate::Error::new(crate::ErrorKind::TrailingData, self.decoder.total_in()),
                    ));
                }
                _ => return Ok(taken),
            }
        }
    }

    /// Checks that the stream ended and returns the destination.
    ///
    /// # Errors
    ///
    /// `UnexpectedEof` when the stream is incomplete; errors of the
    /// destination.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::io::Write;
    ///
    /// use compress_lib::{Format, zlib, io::DecodeWriter};
    ///
    /// let packed = zlib::compress(b"whole");
    /// let mut w = DecodeWriter::new(Vec::new(), Format::Zlib);
    /// w.write_all(&packed[..packed.len() - 2])?;
    /// assert_eq!(w.finish().unwrap_err().kind(), std::io::ErrorKind::UnexpectedEof);
    /// # Ok::<(), std::io::Error>(())
    /// ```
    pub fn finish(mut self) -> io::Result<W> {
        self.pump(&[], true)?;
        self.inner.flush()?;
        Ok(self.inner)
    }
}

impl<W: Write> Write for DecodeWriter<W> {
    fn write(&mut self, data: &[u8]) -> io::Result<usize> {
        self.pump(data, false)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}
