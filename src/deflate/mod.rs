//! Raw DEFLATE (RFC 1951).
//!
//! Tier 1 is one call each way; [`Inflater`] and [`Deflater`] are the
//! streaming engines underneath.
//! Raw DEFLATE has no header and no checksum: use it where the container
//! carries those (WebSocket permessage-deflate, zip entries). For HTTP's
//! `deflate` content coding, which is zlib framing, use `zlib` (0.5).
//!
//! ```
//! use compress_lib::deflate;
//!
//! let packed = deflate::compress(b"hello, hello, hello");
//! assert_eq!(deflate::decompress(&packed)?, b"hello, hello, hello");
//! # Ok::<(), compress_lib::Error>(())
//! ```

mod encoder;
pub(crate) mod huffman;

use alloc::vec::Vec;

pub use encoder::{Deflater, Flush, MemoryConfig};

pub use crate::inflate::Inflater;
pub use crate::stream::{Progress, Status};
use crate::{Error, ErrorKind, Level, Limits};

/// Compresses `data` into a complete raw DEFLATE stream at level 6.
///
/// # Examples
///
/// ```
/// use compress_lib::deflate;
///
/// let packed = deflate::compress(&[b'a'; 1000]);
/// assert!(packed.len() < 20);
/// assert_eq!(deflate::decompress(&packed)?, vec![b'a'; 1000]);
/// # Ok::<(), compress_lib::Error>(())
/// ```
#[must_use]
pub fn compress(data: &[u8]) -> Vec<u8> {
    compress_with(data, Level::DEFAULT)
}

/// Compresses `data` into a complete raw DEFLATE stream at `level`.
///
/// Allocation failure aborts, as for any `Vec`; use [`Deflater`] with a
/// caller-owned buffer to control memory.
///
/// # Examples
///
/// ```
/// use compress_lib::{Level, deflate};
///
/// let data = b"the quick brown fox jumps over the lazy dog, the quick brown fox";
/// let fast = deflate::compress_with(data, Level::FAST);
/// let stored = deflate::compress_with(data, Level::NONE);
/// assert!(fast.len() < stored.len());
/// assert_eq!(deflate::decompress(&stored)?, data);
/// # Ok::<(), compress_lib::Error>(())
/// ```
#[must_use]
pub fn compress_with(data: &[u8], level: Level) -> Vec<u8> {
    let mut deflater = Deflater::new(level);
    let mut out = Vec::with_capacity(data.len() / 2 + 64);
    // `compress_vec` fails only when the vector cannot grow; a `Vec` that
    // cannot grow is an abort anyway, so the output so far is returned.
    let _ = deflater.compress_vec(data, &mut out, Flush::Finish);
    out
}

/// Decompresses a complete raw DEFLATE stream with the default [`Limits`]
/// (64 MiB of output at most).
///
/// # Errors
///
/// Any decoding error; `UnexpectedEnd` if the stream is truncated,
/// `TrailingData` if bytes follow its end, `OutputLimit` past 64 MiB.
///
/// # Examples
///
/// ```
/// use compress_lib::{ErrorKind, deflate};
///
/// assert_eq!(deflate::decompress(&[0x03, 0x00])?, b"");
/// let err = deflate::decompress(&[0x03, 0x00, 0xFF]).unwrap_err();
/// assert_eq!(err.kind(), ErrorKind::TrailingData);
/// # Ok::<(), compress_lib::Error>(())
/// ```
pub fn decompress(data: &[u8]) -> Result<Vec<u8>, Error> {
    decompress_with(data, Limits::new())
}

/// Decompresses a complete raw DEFLATE stream with the given limits.
///
/// # Errors
///
/// As [`decompress`], with the caps of `limits`.
///
/// # Examples
///
/// ```
/// use compress_lib::{ErrorKind, Limits, deflate};
///
/// // 1,000 zero bytes, compressed.
/// let packed = [0x63, 0x60, 0x18, 0x05, 0xA3, 0x60, 0x14, 0x0C, 0x77, 0x00, 0x00];
/// assert_eq!(deflate::decompress(&packed)?.len(), 1000);
/// let tight = Limits::new().with_max_output(999);
/// assert_eq!(deflate::decompress_with(&packed, tight).unwrap_err().kind(), ErrorKind::OutputLimit);
/// # Ok::<(), compress_lib::Error>(())
/// ```
pub fn decompress_with(data: &[u8], limits: Limits) -> Result<Vec<u8>, Error> {
    let mut inflater = Inflater::without_history(limits);
    let cap = usize::try_from(limits.max_output()).unwrap_or(usize::MAX);
    let mut out = Vec::new();
    // A first guess at the size: four times the input, within the cap.
    if out
        .try_reserve(data.len().saturating_mul(4).clamp(64, 1 << 20).min(cap))
        .is_err()
    {
        return Err(Error::config(ErrorKind::AllocationFailed));
    }
    let p = inflater.finish_vec(data, &mut out)?;
    if p.consumed != data.len() {
        return Err(Error::new(ErrorKind::TrailingData, p.consumed as u64));
    }
    Ok(out)
}
