//! Compression levels.

use crate::{Error, ErrorKind};

/// A compression level, 0 to 9, with zlib's meaning: 0 stores without
/// compressing, 1 is the fastest, 9 compresses best, 6 is the default.
///
/// # Examples
///
/// ```
/// use compress_lib::Level;
///
/// assert_eq!(Level::DEFAULT.get(), 6);
/// assert_eq!(Level::new(9)?, Level::BEST);
/// assert!(Level::new(10).is_err());
/// # Ok::<(), compress_lib::Error>(())
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Level(u8);

impl Level {
    /// Level 0: stored blocks, no compression.
    pub const NONE: Level = Level(0);
    /// Level 1: the fastest.
    pub const FAST: Level = Level(1);
    /// Level 6: the default balance of speed and size.
    pub const DEFAULT: Level = Level(6);
    /// Level 9: the smallest output.
    pub const BEST: Level = Level(9);

    /// The level `n`, or `InvalidLevel` above 9.
    ///
    /// # Errors
    ///
    /// [`ErrorKind::InvalidLevel`] when `n > 9`.
    ///
    /// # Examples
    ///
    /// ```
    /// use compress_lib::{ErrorKind, Level};
    ///
    /// assert_eq!(Level::new(1)?, Level::FAST);
    /// assert_eq!(Level::new(12).unwrap_err().kind(), ErrorKind::InvalidLevel);
    /// # Ok::<(), compress_lib::Error>(())
    /// ```
    pub const fn new(n: u8) -> Result<Level, Error> {
        if n <= 9 {
            Ok(Level(n))
        } else {
            Err(Error::config(ErrorKind::InvalidLevel))
        }
    }

    /// The number, 0 to 9.
    ///
    /// # Examples
    ///
    /// ```
    /// assert_eq!(compress_lib::Level::BEST.get(), 9);
    /// ```
    #[must_use]
    pub const fn get(self) -> u8 {
        self.0
    }
}

impl Default for Level {
    fn default() -> Self {
        Self::DEFAULT
    }
}

impl TryFrom<u8> for Level {
    type Error = Error;

    fn try_from(n: u8) -> Result<Self, Error> {
        Level::new(n)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_level_every_value_zero_to_nine_valid() {
        for n in 0..=9 {
            assert_eq!(Level::new(n).map(Level::get), Ok(n));
        }
    }

    #[test]
    fn test_level_ten_and_above_invalid() {
        for n in 10..=255u8 {
            assert_eq!(
                Level::new(n).map_err(|e| e.kind()),
                Err(ErrorKind::InvalidLevel)
            );
            assert!(Level::try_from(n).is_err());
        }
    }

    #[test]
    fn test_level_constants_and_default() {
        assert_eq!(Level::NONE.get(), 0);
        assert_eq!(Level::FAST.get(), 1);
        assert_eq!(Level::default(), Level::DEFAULT);
        assert!(Level::FAST < Level::BEST);
    }
}
