//! Per-message compression for WebSocket permessage-deflate (RFC 7692).
//!
//! A message is compressed with a sync flush and sent without the trailing
//! `00 00 FF FF`; the receiver appends those four bytes and inflates. With
//! context takeover the window carries over between messages; without it
//! every message stands alone (the safer default: secrets in one message
//! cannot leak through the compressed size of the next, RFC 7692 section 8).
//!
//! This module does the codec part only. Negotiating
//! `Sec-WebSocket-Extensions`, the RSV1 bit, and the per-message opt-out
//! belong to the WebSocket layer.
//!
//! ```
//! use compress_lib::message::{Compressor, Decompressor, MessageConfig};
//!
//! let config = MessageConfig::new().with_window_bits(12);
//! let mut tx = Compressor::new(config)?;
//! let mut rx = Decompressor::new(config)?;
//! let mut wire = Vec::new();
//! tx.compress_message(b"{\"hello\":\"world\"}", &mut wire)?;
//! let mut text = Vec::new();
//! rx.decompress_message(&wire, &mut text, 1 << 20)?;
//! assert_eq!(text, b"{\"hello\":\"world\"}");
//! # Ok::<(), compress_lib::Error>(())
//! ```

use alloc::vec::Vec;

use crate::{
    Error, ErrorKind, Level,
    deflate::{Deflater, Flush, Inflater},
};

/// The four bytes a sync flush ends with, removed by the sender and put
/// back by the receiver.
const TAIL: [u8; 4] = [0x00, 0x00, 0xFF, 0xFF];

/// Settings for one direction of a permessage-deflate connection.
///
/// | Setting | Default | RFC 7692 parameter |
/// |---|---|---|
/// | window bits | 15 | `server_max_window_bits` / `client_max_window_bits` |
/// | context takeover | off | `server_no_context_takeover` / `client_no_context_takeover` absent |
/// | level | 1 | (local choice) |
///
/// Memory per direction: without takeover a [`Decompressor`] holds only its
/// decode tables (about 11 KiB, no window); with takeover it adds the window
/// (`2^window_bits` bytes). A [`Compressor`] holds a deflater sized by the
/// window bits (about 270 KiB at 15, under 100 KiB at 9).
///
/// # Examples
///
/// ```
/// use compress_lib::{Level, message::MessageConfig};
///
/// let config = MessageConfig::new().with_window_bits(10).with_context_takeover(true).with_level(Level::DEFAULT);
/// assert_eq!(config.window_bits(), 10);
/// assert!(config.context_takeover());
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub struct MessageConfig {
    window_bits: u8,
    context_takeover: bool,
    level: Level,
}

impl MessageConfig {
    /// Window bits 15, no context takeover, level 1.
    ///
    /// # Examples
    ///
    /// ```
    /// use compress_lib::message::MessageConfig;
    ///
    /// assert_eq!(MessageConfig::new(), MessageConfig::default());
    /// assert!(!MessageConfig::new().context_takeover());
    /// ```
    #[must_use]
    pub const fn new() -> Self {
        Self {
            window_bits: 15,
            context_takeover: false,
            level: Level::FAST,
        }
    }

    /// Sets the window bits, 8 to 15 (checked when an engine is built).
    ///
    /// # Examples
    ///
    /// ```
    /// assert_eq!(compress_lib::message::MessageConfig::new().with_window_bits(9).window_bits(), 9);
    /// ```
    #[must_use]
    pub const fn with_window_bits(mut self, bits: u8) -> Self {
        self.window_bits = bits;
        self
    }

    /// Keeps the window from one message to the next.
    ///
    /// # Examples
    ///
    /// ```
    /// assert!(compress_lib::message::MessageConfig::new().with_context_takeover(true).context_takeover());
    /// ```
    #[must_use]
    pub const fn with_context_takeover(mut self, on: bool) -> Self {
        self.context_takeover = on;
        self
    }

    /// Sets the compression level (the compressor only).
    ///
    /// # Examples
    ///
    /// ```
    /// use compress_lib::{Level, message::MessageConfig};
    ///
    /// assert_eq!(MessageConfig::new().with_level(Level::BEST).level(), Level::BEST);
    /// ```
    #[must_use]
    pub const fn with_level(mut self, level: Level) -> Self {
        self.level = level;
        self
    }

    /// The window bits.
    ///
    /// # Examples
    ///
    /// ```
    /// assert_eq!(compress_lib::message::MessageConfig::new().window_bits(), 15);
    /// ```
    #[must_use]
    pub const fn window_bits(&self) -> u8 {
        self.window_bits
    }

    /// Whether the window carries over between messages.
    ///
    /// # Examples
    ///
    /// ```
    /// assert!(!compress_lib::message::MessageConfig::new().context_takeover());
    /// ```
    #[must_use]
    pub const fn context_takeover(&self) -> bool {
        self.context_takeover
    }

    /// The compression level.
    ///
    /// # Examples
    ///
    /// ```
    /// use compress_lib::{Level, message::MessageConfig};
    ///
    /// assert_eq!(MessageConfig::new().level(), Level::FAST);
    /// ```
    #[must_use]
    pub const fn level(&self) -> Level {
        self.level
    }
}

impl Default for MessageConfig {
    fn default() -> Self {
        Self::new()
    }
}

/// Compresses messages for permessage-deflate.
///
/// # Examples
///
/// ```
/// use compress_lib::message::{Compressor, MessageConfig};
///
/// let mut tx = Compressor::new(MessageConfig::new())?;
/// let mut wire = Vec::new();
/// tx.compress_message(b"Hello", &mut wire)?;
/// assert_eq!(wire, [0xF2, 0x48, 0xCD, 0xC9, 0xC9, 0x07, 0x00]); // RFC 7692 7.2.3.1
/// # Ok::<(), compress_lib::Error>(())
/// ```
#[derive(Debug)]
pub struct Compressor {
    deflater: Deflater,
    takeover: bool,
}

impl Compressor {
    /// A compressor for one direction of a connection.
    ///
    /// # Errors
    ///
    /// [`ErrorKind::InvalidWindowBits`] outside 8 to 15.
    ///
    /// # Examples
    ///
    /// ```
    /// use compress_lib::message::{Compressor, MessageConfig};
    ///
    /// assert!(Compressor::new(MessageConfig::new().with_window_bits(8)).is_ok());
    /// assert!(Compressor::new(MessageConfig::new().with_window_bits(16)).is_err());
    /// ```
    pub fn new(config: MessageConfig) -> Result<Self, Error> {
        let deflater = Deflater::new(config.level).with_window_bits(config.window_bits)?;
        Ok(Self {
            deflater,
            takeover: config.context_takeover,
        })
    }

    /// Compresses one message, appending the payload (without the trailing
    /// `00 00 FF FF`) to `out`. Returns the payload length. An empty
    /// message compresses to the single byte `00`.
    ///
    /// The payload can be longer than the message for incompressible data;
    /// the WebSocket layer may then send the message uncompressed instead
    /// (with takeover the compressor's window has advanced either way, so
    /// call [`reset`](Compressor::reset) and tell the peer, or keep sending
    /// compressed).
    ///
    /// # Errors
    ///
    /// [`ErrorKind::AllocationFailed`] when `out` cannot grow.
    ///
    /// # Examples
    ///
    /// ```
    /// use compress_lib::message::{Compressor, MessageConfig};
    ///
    /// let mut tx = Compressor::new(MessageConfig::new())?;
    /// let mut wire = Vec::new();
    /// assert_eq!(tx.compress_message(b"", &mut wire)?, 1);
    /// assert_eq!(wire, [0x00]);
    /// # Ok::<(), compress_lib::Error>(())
    /// ```
    pub fn compress_message(&mut self, message: &[u8], out: &mut Vec<u8>) -> Result<usize, Error> {
        let start = out.len();
        let result = self.deflater.compress_vec(message, out, Flush::Sync);
        if let Err(e) = result {
            out.truncate(start);
            self.deflater.reset();
            return Err(e);
        }
        // A sync flush always ends with the empty stored block's length
        // bytes; take them off.
        if out.len() >= start + TAIL.len() && out[out.len() - TAIL.len()..] == TAIL {
            out.truncate(out.len() - TAIL.len());
        }
        if !self.takeover {
            self.deflater.reset();
        }
        Ok(out.len() - start)
    }

    /// Forgets the window (for a connection that restarts its context).
    ///
    /// # Examples
    ///
    /// ```
    /// use compress_lib::message::{Compressor, MessageConfig};
    ///
    /// let mut tx = Compressor::new(MessageConfig::new().with_context_takeover(true))?;
    /// tx.reset();
    /// # Ok::<(), compress_lib::Error>(())
    /// ```
    pub fn reset(&mut self) {
        self.deflater.reset();
    }

    /// Heap bytes held.
    ///
    /// # Examples
    ///
    /// ```
    /// use compress_lib::message::{Compressor, MessageConfig};
    ///
    /// let small = Compressor::new(MessageConfig::new().with_window_bits(9))?;
    /// let big = Compressor::new(MessageConfig::new())?;
    /// assert!(small.memory_in_use() < big.memory_in_use());
    /// # Ok::<(), compress_lib::Error>(())
    /// ```
    #[must_use]
    pub fn memory_in_use(&self) -> usize {
        self.deflater.memory_in_use()
    }
}

/// Decompresses permessage-deflate messages.
///
/// The output budget is checked per output chunk while inflating, never
/// after, so a compressed bomb stops at the budget; the error is
/// [`ErrorKind::OutputLimit`] (WebSocket close code 1009). Undecodable data
/// is any other kind (close code 1007).
///
/// # Examples
///
/// ```
/// use compress_lib::{ErrorKind, message::{Decompressor, MessageConfig}};
///
/// let mut rx = Decompressor::new(MessageConfig::new())?;
/// let mut text = Vec::new();
/// rx.decompress_message(&[0xF2, 0x48, 0xCD, 0xC9, 0xC9, 0x07, 0x00], &mut text, 1024)?;
/// assert_eq!(text, b"Hello");
/// let err = rx.decompress_message(&[0xF2, 0x48, 0xCD, 0xC9, 0xC9, 0x07, 0x00], &mut Vec::new(), 4).unwrap_err();
/// assert_eq!(err.kind(), ErrorKind::OutputLimit);
/// # Ok::<(), compress_lib::Error>(())
/// ```
#[derive(Debug)]
pub struct Decompressor {
    inflater: Inflater,
    takeover: bool,
    /// Where the current message starts in the caller's vector, while a
    /// fragmented message is open.
    open: Option<OpenMessage>,
}

#[derive(Clone, Copy, Debug)]
struct OpenMessage {
    start: usize,
    produced: usize,
    max: usize,
}

impl Decompressor {
    /// A decompressor for one direction of a connection.
    ///
    /// # Errors
    ///
    /// [`ErrorKind::InvalidWindowBits`] outside 8 to 15.
    ///
    /// # Examples
    ///
    /// ```
    /// use compress_lib::message::{Decompressor, MessageConfig};
    ///
    /// assert!(Decompressor::new(MessageConfig::new().with_window_bits(8)).is_ok());
    /// ```
    pub fn new(config: MessageConfig) -> Result<Self, Error> {
        let inflater = if config.context_takeover {
            Inflater::new().with_window_bits(config.window_bits)?
        } else {
            // Each message is decoded into one vector, which is its own
            // history: no window is kept.
            Inflater::without_history(crate::Limits::unlimited())
                .with_window_bits(config.window_bits)?
        };
        Ok(Self {
            inflater,
            takeover: config.context_takeover,
            open: None,
        })
    }

    /// Decompresses one whole message payload, appending to `out`, with at
    /// most `max` bytes of output. Returns the bytes appended.
    ///
    /// # Errors
    ///
    /// `OutputLimit` past `max`; `TrailingData` for nonzero bytes after a
    /// final block; any decoding error. After an error the decompressor is reset
    /// (the connection should be failed anyway).
    ///
    /// # Examples
    ///
    /// ```
    /// use compress_lib::message::{Decompressor, MessageConfig};
    ///
    /// let mut rx = Decompressor::new(MessageConfig::new())?;
    /// let mut out = Vec::new();
    /// assert_eq!(rx.decompress_message(&[0x00], &mut out, 16)?, 0); // empty message
    /// # Ok::<(), compress_lib::Error>(())
    /// ```
    pub fn decompress_message(
        &mut self,
        payload: &[u8],
        out: &mut Vec<u8>,
        max: usize,
    ) -> Result<usize, Error> {
        self.decompress_fragment(payload, true, out, max)
    }

    /// Decompresses one frame of a message, appending to `out`; `last` marks
    /// the final frame. Every fragment of a message must append to the same
    /// vector, untouched in between: the vector is the message's history.
    /// `max` caps the whole message. Returns the bytes appended by this
    /// fragment.
    ///
    /// # Errors
    ///
    /// As [`decompress_message`](Decompressor::decompress_message), plus
    /// `InvalidState` when `out` was changed between fragments.
    ///
    /// # Examples
    ///
    /// ```
    /// use compress_lib::message::{Decompressor, MessageConfig};
    ///
    /// let payload = [0xF2, 0x48, 0xCD, 0xC9, 0xC9, 0x07, 0x00];
    /// let mut rx = Decompressor::new(MessageConfig::new())?;
    /// let mut out = Vec::new();
    /// rx.decompress_fragment(&payload[..3], false, &mut out, 1024)?;
    /// rx.decompress_fragment(&payload[3..], true, &mut out, 1024)?;
    /// assert_eq!(out, b"Hello");
    /// # Ok::<(), compress_lib::Error>(())
    /// ```
    pub fn decompress_fragment(
        &mut self,
        fragment: &[u8],
        last: bool,
        out: &mut Vec<u8>,
        max: usize,
    ) -> Result<usize, Error> {
        let result = self.fragment(fragment, last, out, max);
        if result.is_err() {
            self.open = None;
            self.inflater.reset();
        }
        result
    }

    fn fragment(
        &mut self,
        fragment: &[u8],
        last: bool,
        out: &mut Vec<u8>,
        max: usize,
    ) -> Result<usize, Error> {
        let open = match self.open {
            Some(m) => {
                if out.len() != m.start + m.produced {
                    return Err(Error::config(ErrorKind::InvalidState));
                }
                m
            }
            None => OpenMessage {
                start: out.len(),
                produced: 0,
                max,
            },
        };
        let before = out.len();
        let base = self.inflater.total_out() - open.produced as u64;
        self.inflater
            .set_max_output(base.saturating_add(open.max as u64));

        let p = self
            .inflater
            .decompress_into(fragment, out, open.start, false)?;
        // After a final block only zero padding may follow (RFC 7692's own
        // example in section 7.2.3.4 ends with one such byte); anything else
        // is refused.
        if self.inflater.is_done() && fragment[p.consumed..].iter().any(|&b| b != 0) {
            return Err(Error::new(
                ErrorKind::TrailingData,
                self.inflater.total_in(),
            ));
        }
        if last && !self.inflater.is_done() {
            self.inflater
                .decompress_into(&TAIL, out, open.start, false)?;
        }
        let produced = open.produced + (out.len() - before);
        if !last {
            self.open = Some(OpenMessage { produced, ..open });
            return Ok(out.len() - before);
        }
        self.open = None;
        if self.takeover {
            self.inflater.remember(&out[open.start..]);
            if self.inflater.is_done() {
                self.inflater.next_stream()?;
            }
        } else {
            self.inflater.reset();
        }
        Ok(out.len() - before)
    }

    /// Forgets the window and any open message.
    ///
    /// # Examples
    ///
    /// ```
    /// use compress_lib::message::{Decompressor, MessageConfig};
    ///
    /// let mut rx = Decompressor::new(MessageConfig::new())?;
    /// rx.reset();
    /// # Ok::<(), compress_lib::Error>(())
    /// ```
    pub fn reset(&mut self) {
        self.open = None;
        self.inflater.reset();
    }

    /// Heap bytes held: the decode tables, plus the window with takeover.
    ///
    /// # Examples
    ///
    /// ```
    /// use compress_lib::message::{Decompressor, MessageConfig};
    ///
    /// let lean = Decompressor::new(MessageConfig::new())?;
    /// let takeover = Decompressor::new(MessageConfig::new().with_context_takeover(true))?;
    /// assert!(lean.memory_in_use() + 32 * 1024 <= takeover.memory_in_use());
    /// # Ok::<(), compress_lib::Error>(())
    /// ```
    #[must_use]
    pub fn memory_in_use(&self) -> usize {
        self.inflater.memory_in_use()
    }
}
