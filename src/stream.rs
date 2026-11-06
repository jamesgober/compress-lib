//! The result of one streaming call.

/// Where a streaming engine stopped.
///
/// # Examples
///
/// ```
/// use compress_lib::deflate::{Inflater, Status};
///
/// let mut out = [0u8; 8];
/// let p = Inflater::new().decompress(&[0xCB, 0x48], &mut out)?;
/// assert_eq!(p.status, Status::NeedsInput);
/// # Ok::<(), compress_lib::Error>(())
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Status {
    /// All input was consumed and the stream goes on: call again with more.
    NeedsInput,
    /// The output buffer is full: call again with more room and the input
    /// not yet consumed.
    NeedsOutput,
    /// The stream (or, for encoders, the requested flush) is complete.
    Done,
}

/// What one streaming call did.
///
/// # Examples
///
/// ```
/// use compress_lib::deflate::{Inflater, Status};
///
/// let mut out = [0u8; 8];
/// let p = Inflater::new().finish(&[0xCB, 0x48, 0xCD, 0xC9, 0xC9, 0x07, 0x00], &mut out)?;
/// assert_eq!((p.consumed, p.produced, p.status), (7, 5, Status::Done));
/// # Ok::<(), compress_lib::Error>(())
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub struct Progress {
    /// Bytes of input consumed (never more than the input given).
    pub consumed: usize,
    /// Bytes of output produced (never more than the room given).
    pub produced: usize,
    /// Where the engine stopped.
    pub status: Status,
}

impl Progress {
    pub(crate) const fn new(consumed: usize, produced: usize, status: Status) -> Self {
        Self {
            consumed,
            produced,
            status,
        }
    }
}
