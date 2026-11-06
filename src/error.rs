//! The error type every codec returns.

use core::fmt;

/// What went wrong, as a stable, matchable value.
///
/// Every kind says what to do in its [`message`](ErrorKind::message) and has a
/// stable [`code`](ErrorKind::code) for logs and metrics. New kinds may be
/// added in minor releases, so match with a wildcard arm.
///
/// # Examples
///
/// ```
/// use compress_lib::ErrorKind;
///
/// assert_eq!(ErrorKind::OutputLimit.code(), "compress.output_limit");
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum ErrorKind {
    /// A DEFLATE block header names block type 3, which does not exist.
    InvalidBlockType,
    /// A stored block's length and its one's complement disagree.
    InvalidStoredLength,
    /// A Huffman code description is invalid: over-subscribed, incomplete
    /// where the format forbids it, a repeat with no previous length, or a
    /// repeat that runs past the end of the table.
    InvalidCodeLengths,
    /// A code decodes to a symbol the format reserves (length symbols 286
    /// and 287, distance symbols 30 and 31) or to no symbol at all.
    InvalidSymbol,
    /// A back-reference points before the start of the data (and any preset
    /// dictionary) or further back than the window allows.
    DistanceTooFar,
    /// The input ended inside the stream: the data is truncated.
    UnexpectedEnd,
    /// The output reached the configured `max_output` before the stream
    /// ended.
    OutputLimit,
    /// The output grew past the configured ratio to the input consumed.
    RatioLimit,
    /// Bytes follow the end of the stream where none are allowed.
    TrailingData,
    /// A window size outside 8 to 15 bits was asked for, or a zlib header
    /// names a window larger than the decoder allows.
    InvalidWindowBits,
    /// A compression level outside 0 to 9 was asked for.
    InvalidLevel,
    /// A zlib or gzip header is malformed.
    InvalidHeader,
    /// A checksum (Adler-32, CRC-32, or the gzip header CRC) does not match
    /// the data.
    ChecksumMismatch,
    /// The gzip trailer's length does not match the data.
    LengthMismatch,
    /// The gzip header's optional fields are larger than the configured cap.
    HeaderTooLarge,
    /// A gzip stream has more members than the configured cap.
    TooManyMembers,
    /// A zlib stream needs a preset dictionary that was not given.
    DictionaryRequired,
    /// The preset dictionary given is not the one the zlib stream names.
    WrongDictionary,
    /// The call is not valid in the engine's current state.
    InvalidState,
    /// The allocator refused memory for output.
    AllocationFailed,
}

impl ErrorKind {
    /// A stable identifier for logs and metrics, never changed once
    /// released.
    ///
    /// # Examples
    ///
    /// ```
    /// use compress_lib::ErrorKind;
    ///
    /// assert_eq!(ErrorKind::DistanceTooFar.code(), "compress.distance_too_far");
    /// ```
    #[must_use]
    pub const fn code(self) -> &'static str {
        match self {
            Self::InvalidBlockType => "compress.invalid_block_type",
            Self::InvalidStoredLength => "compress.invalid_stored_length",
            Self::InvalidCodeLengths => "compress.invalid_code_lengths",
            Self::InvalidSymbol => "compress.invalid_symbol",
            Self::DistanceTooFar => "compress.distance_too_far",
            Self::UnexpectedEnd => "compress.unexpected_end",
            Self::OutputLimit => "compress.output_limit",
            Self::RatioLimit => "compress.ratio_limit",
            Self::TrailingData => "compress.trailing_data",
            Self::InvalidWindowBits => "compress.invalid_window_bits",
            Self::InvalidLevel => "compress.invalid_level",
            Self::InvalidHeader => "compress.invalid_header",
            Self::ChecksumMismatch => "compress.checksum_mismatch",
            Self::LengthMismatch => "compress.length_mismatch",
            Self::HeaderTooLarge => "compress.header_too_large",
            Self::TooManyMembers => "compress.too_many_members",
            Self::DictionaryRequired => "compress.dictionary_required",
            Self::WrongDictionary => "compress.wrong_dictionary",
            Self::InvalidState => "compress.invalid_state",
            Self::AllocationFailed => "compress.allocation_failed",
        }
    }

    /// What happened and what to do, in one sentence.
    ///
    /// # Examples
    ///
    /// ```
    /// use compress_lib::ErrorKind;
    ///
    /// assert!(ErrorKind::OutputLimit.message().contains("max_output"));
    /// ```
    #[must_use]
    pub const fn message(self) -> &'static str {
        match self {
            Self::InvalidBlockType => {
                "invalid DEFLATE block type 3: the data is not DEFLATE or is corrupt"
            }
            Self::InvalidStoredLength => {
                "stored block length check failed: the data is corrupt or truncated"
            }
            Self::InvalidCodeLengths => {
                "invalid Huffman code lengths: the data is corrupt or not DEFLATE"
            }
            Self::InvalidSymbol => "invalid or reserved symbol: the data is corrupt",
            Self::DistanceTooFar => {
                "back-reference beyond the data or the window: the data is corrupt, \
                 needs a preset dictionary, or uses a larger window than configured"
            }
            Self::UnexpectedEnd => "input ended inside the stream: the data is truncated",
            Self::OutputLimit => {
                "output reached max_output: raise the limit only if the source is trusted"
            }
            Self::RatioLimit => {
                "output exceeded the ratio limit: likely a decompression bomb; \
                 raise max_ratio only if the source is trusted"
            }
            Self::TrailingData => {
                "bytes follow the end of the stream: strip them or allow trailing data"
            }
            Self::InvalidWindowBits => "window bits must be 8 to 15 (and within the decoder's)",
            Self::InvalidLevel => "compression level must be 0 to 9",
            Self::InvalidHeader => "invalid zlib or gzip header: the data is not in that format",
            Self::ChecksumMismatch => "checksum mismatch: the data is corrupt",
            Self::LengthMismatch => "gzip length mismatch: the data is corrupt or truncated",
            Self::HeaderTooLarge => {
                "gzip header fields exceed the configured cap: raise max_header if trusted"
            }
            Self::TooManyMembers => {
                "gzip stream has more members than max_members: raise it if trusted"
            }
            Self::DictionaryRequired => {
                "the zlib stream needs a preset dictionary: call set_dictionary first"
            }
            Self::WrongDictionary => "wrong preset dictionary: use the one the zlib stream names",
            Self::InvalidState => "call not valid in the engine's state: reset it or finish first",
            Self::AllocationFailed => "out of memory for output: lower max_output or free memory",
        }
    }
}

impl fmt::Display for ErrorKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.message())
    }
}

/// A codec error: its [`ErrorKind`] and the input offset where it was found.
///
/// # Examples
///
/// ```
/// use compress_lib::{Error, ErrorKind};
///
/// fn check(e: Error) -> bool {
///     matches!(e.kind(), ErrorKind::OutputLimit | ErrorKind::RatioLimit)
/// }
/// # let _ = check;
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Error {
    kind: ErrorKind,
    offset: u64,
}

impl Error {
    /// An error of `kind` found at input byte `offset`.
    #[must_use]
    pub(crate) const fn new(kind: ErrorKind, offset: u64) -> Self {
        Self { kind, offset }
    }

    /// An error not tied to a place in the input (configuration, state).
    #[must_use]
    pub(crate) const fn config(kind: ErrorKind) -> Self {
        Self { kind, offset: 0 }
    }

    /// What went wrong.
    ///
    /// # Examples
    ///
    /// ```
    /// use compress_lib::{ErrorKind, Level};
    ///
    /// assert_eq!(Level::new(10).unwrap_err().kind(), ErrorKind::InvalidLevel);
    /// ```
    #[must_use]
    pub const fn kind(&self) -> ErrorKind {
        self.kind
    }

    /// The offset in the compressed input (bytes since the engine started or
    /// was last reset) where the problem was found; 0 for configuration
    /// errors.
    ///
    /// # Examples
    ///
    /// ```
    /// use compress_lib::Level;
    ///
    /// assert_eq!(Level::new(10).unwrap_err().offset(), 0);
    /// ```
    #[must_use]
    pub const fn offset(&self) -> u64 {
        self.offset
    }

    /// The stable identifier of the kind, as [`ErrorKind::code`].
    ///
    /// # Examples
    ///
    /// ```
    /// use compress_lib::Level;
    ///
    /// assert_eq!(Level::new(10).unwrap_err().code(), "compress.invalid_level");
    /// ```
    #[must_use]
    pub const fn code(&self) -> &'static str {
        self.kind.code()
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.offset == 0 {
            f.write_str(self.kind.message())
        } else {
            write!(f, "{} (at input byte {})", self.kind.message(), self.offset)
        }
    }
}

impl core::error::Error for Error {}

impl From<Error> for ErrorKind {
    fn from(e: Error) -> Self {
        e.kind
    }
}

#[cfg(feature = "std")]
impl From<Error> for std::io::Error {
    fn from(e: Error) -> Self {
        let kind = match e.kind {
            ErrorKind::UnexpectedEnd => std::io::ErrorKind::UnexpectedEof,
            ErrorKind::InvalidWindowBits | ErrorKind::InvalidLevel | ErrorKind::InvalidState => {
                std::io::ErrorKind::InvalidInput
            }
            ErrorKind::AllocationFailed => std::io::ErrorKind::OutOfMemory,
            _ => std::io::ErrorKind::InvalidData,
        };
        std::io::Error::new(kind, e)
    }
}

#[cfg(test)]
mod tests {
    use alloc::{collections::BTreeSet, string::ToString};

    use super::*;

    const ALL: [ErrorKind; 20] = [
        ErrorKind::InvalidBlockType,
        ErrorKind::InvalidStoredLength,
        ErrorKind::InvalidCodeLengths,
        ErrorKind::InvalidSymbol,
        ErrorKind::DistanceTooFar,
        ErrorKind::UnexpectedEnd,
        ErrorKind::OutputLimit,
        ErrorKind::RatioLimit,
        ErrorKind::TrailingData,
        ErrorKind::InvalidWindowBits,
        ErrorKind::InvalidLevel,
        ErrorKind::InvalidHeader,
        ErrorKind::ChecksumMismatch,
        ErrorKind::LengthMismatch,
        ErrorKind::HeaderTooLarge,
        ErrorKind::TooManyMembers,
        ErrorKind::DictionaryRequired,
        ErrorKind::WrongDictionary,
        ErrorKind::InvalidState,
        ErrorKind::AllocationFailed,
    ];

    #[test]
    fn test_error_codes_unique_and_prefixed() {
        let codes: BTreeSet<_> = ALL.iter().map(|k| k.code()).collect();
        assert_eq!(codes.len(), ALL.len());
        assert!(codes.iter().all(|c| c.starts_with("compress.")));
    }

    #[test]
    fn test_error_messages_say_what_to_do() {
        for k in ALL {
            let m = k.message();
            assert!(m.contains(':') || m.contains("must"), "{k:?}: {m}");
            assert_eq!(k.to_string(), m);
        }
    }

    #[test]
    fn test_error_display_includes_offset_when_known() {
        let e = Error::new(ErrorKind::InvalidBlockType, 17);
        assert!(e.to_string().ends_with("(at input byte 17)"));
        assert_eq!(e.offset(), 17);
        assert_eq!(e.kind(), ErrorKind::InvalidBlockType);
        assert_eq!(ErrorKind::from(e), ErrorKind::InvalidBlockType);
        let c = Error::config(ErrorKind::InvalidLevel);
        assert_eq!(c.to_string(), ErrorKind::InvalidLevel.message());
        assert_eq!(c.code(), "compress.invalid_level");
    }

    #[cfg(feature = "std")]
    #[test]
    fn test_error_into_io_error_maps_kind() {
        let io: std::io::Error = Error::new(ErrorKind::UnexpectedEnd, 3).into();
        assert_eq!(io.kind(), std::io::ErrorKind::UnexpectedEof);
        let io: std::io::Error = Error::config(ErrorKind::InvalidLevel).into();
        assert_eq!(io.kind(), std::io::ErrorKind::InvalidInput);
        let io: std::io::Error = Error::new(ErrorKind::ChecksumMismatch, 9).into();
        assert_eq!(io.kind(), std::io::ErrorKind::InvalidData);
    }
}
