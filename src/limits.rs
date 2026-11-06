//! Decoder limits: the defence against decompression bombs.

/// Caps a decoder enforces while it decodes.
///
/// Every check runs per output chunk, before the chunk is written, so a
/// stream that would pass a cap stops at the cap; a decompression bomb costs
/// at most `max_output` bytes of output and the time to produce them.
///
/// | Limit | Default | Cost |
/// |---|---|---|
/// | `max_output` | 64 MiB | one compare per output chunk |
/// | `max_ratio` | off | one multiply and compare per output chunk |
/// | `ratio_floor` | 1 MiB | (the ratio is not checked below it) |
/// | `max_header` (gzip) | 64 KiB | one counter per header byte |
/// | `max_members` (gzip) | 1024 | one counter per member |
///
/// The ratio cap is off by default: DEFLATE's largest possible ratio is
/// about 1032 to 1, legitimate data (zero-filled images, repeated logs)
/// reaches hundreds to one, and the absolute cap is what bounds memory. Turn
/// it on when you know your data, for example at 50 for JSON APIs.
///
/// # Examples
///
/// ```
/// use compress_lib::Limits;
///
/// let limits = Limits::new().with_max_output(1 << 20).with_max_ratio(50);
/// assert_eq!(limits.max_output(), 1 << 20);
/// assert_eq!(limits.max_ratio(), 50);
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub struct Limits {
    max_output: u64,
    max_ratio: u32,
    ratio_floor: u64,
    max_header: usize,
    max_members: u32,
}

impl Limits {
    /// The default absolute output cap: 64 MiB.
    pub const DEFAULT_MAX_OUTPUT: u64 = 64 << 20;
    /// The default output size below which the ratio is not checked: 1 MiB.
    pub const DEFAULT_RATIO_FLOOR: u64 = 1 << 20;
    /// The default cap on gzip optional header fields together: 64 KiB.
    pub const DEFAULT_MAX_HEADER: usize = 64 << 10;
    /// The default cap on gzip members in one stream: 1024.
    pub const DEFAULT_MAX_MEMBERS: u32 = 1024;

    /// The safe defaults (see the table on [`Limits`]).
    ///
    /// # Examples
    ///
    /// ```
    /// use compress_lib::Limits;
    ///
    /// assert_eq!(Limits::new().max_output(), 64 << 20);
    /// assert_eq!(Limits::new(), Limits::default());
    /// ```
    #[must_use]
    pub const fn new() -> Self {
        Self {
            max_output: Self::DEFAULT_MAX_OUTPUT,
            max_ratio: 0,
            ratio_floor: Self::DEFAULT_RATIO_FLOOR,
            max_header: Self::DEFAULT_MAX_HEADER,
            max_members: Self::DEFAULT_MAX_MEMBERS,
        }
    }

    /// No output cap and no ratio cap, for trusted data such as your own
    /// files. Named so that its use stands out in review.
    ///
    /// # Examples
    ///
    /// ```
    /// use compress_lib::Limits;
    ///
    /// assert_eq!(Limits::unlimited().max_output(), u64::MAX);
    /// ```
    #[must_use]
    pub const fn unlimited() -> Self {
        Self {
            max_output: u64::MAX,
            max_ratio: 0,
            ratio_floor: Self::DEFAULT_RATIO_FLOOR,
            max_header: Self::DEFAULT_MAX_HEADER,
            max_members: u32::MAX,
        }
    }

    /// Sets the total output a decoder may produce.
    ///
    /// # Examples
    ///
    /// ```
    /// let limits = compress_lib::Limits::new().with_max_output(16 << 20);
    /// assert_eq!(limits.max_output(), 16 << 20);
    /// ```
    #[must_use]
    pub const fn with_max_output(mut self, bytes: u64) -> Self {
        self.max_output = bytes;
        self
    }

    /// Sets the largest allowed ratio of output to input consumed, checked
    /// once the output passes the ratio floor. `0` turns the check off.
    ///
    /// # Examples
    ///
    /// ```
    /// let limits = compress_lib::Limits::new().with_max_ratio(100);
    /// assert_eq!(limits.max_ratio(), 100);
    /// ```
    #[must_use]
    pub const fn with_max_ratio(mut self, ratio: u32) -> Self {
        self.max_ratio = ratio;
        self
    }

    /// Sets the output size below which the ratio is not checked (small
    /// inputs legitimately have high ratios).
    ///
    /// # Examples
    ///
    /// ```
    /// let limits = compress_lib::Limits::new().with_ratio_floor(64 << 10);
    /// assert_eq!(limits.ratio_floor(), 64 << 10);
    /// ```
    #[must_use]
    pub const fn with_ratio_floor(mut self, bytes: u64) -> Self {
        self.ratio_floor = bytes;
        self
    }

    /// Sets the cap on the gzip header's optional fields (FEXTRA, FNAME,
    /// FCOMMENT) together.
    ///
    /// # Examples
    ///
    /// ```
    /// let limits = compress_lib::Limits::new().with_max_header(1024);
    /// assert_eq!(limits.max_header(), 1024);
    /// ```
    #[must_use]
    pub const fn with_max_header(mut self, bytes: usize) -> Self {
        self.max_header = bytes;
        self
    }

    /// Sets the cap on members in one gzip stream.
    ///
    /// # Examples
    ///
    /// ```
    /// let limits = compress_lib::Limits::new().with_max_members(1);
    /// assert_eq!(limits.max_members(), 1);
    /// ```
    #[must_use]
    pub const fn with_max_members(mut self, members: u32) -> Self {
        self.max_members = members;
        self
    }

    /// The total output cap.
    ///
    /// # Examples
    ///
    /// ```
    /// assert_eq!(compress_lib::Limits::new().max_output(), 64 << 20);
    /// ```
    #[must_use]
    pub const fn max_output(&self) -> u64 {
        self.max_output
    }

    /// The ratio cap (0 = off).
    ///
    /// # Examples
    ///
    /// ```
    /// assert_eq!(compress_lib::Limits::new().max_ratio(), 0);
    /// ```
    #[must_use]
    pub const fn max_ratio(&self) -> u32 {
        self.max_ratio
    }

    /// The output size below which the ratio is not checked.
    ///
    /// # Examples
    ///
    /// ```
    /// assert_eq!(compress_lib::Limits::new().ratio_floor(), 1 << 20);
    /// ```
    #[must_use]
    pub const fn ratio_floor(&self) -> u64 {
        self.ratio_floor
    }

    /// The gzip header field cap.
    ///
    /// # Examples
    ///
    /// ```
    /// assert_eq!(compress_lib::Limits::new().max_header(), 64 << 10);
    /// ```
    #[must_use]
    pub const fn max_header(&self) -> usize {
        self.max_header
    }

    /// The gzip member cap.
    ///
    /// # Examples
    ///
    /// ```
    /// assert_eq!(compress_lib::Limits::new().max_members(), 1024);
    /// ```
    #[must_use]
    pub const fn max_members(&self) -> u32 {
        self.max_members
    }

    /// `true` when `output` bytes from `input` bytes break the ratio cap.
    #[cfg(test)]
    #[must_use]
    pub(crate) const fn ratio_exceeded(&self, output: u64, input: u64) -> bool {
        self.max_ratio != 0
            && output > self.ratio_floor
            && output > input.saturating_mul(self.max_ratio as u64)
    }
}

impl Default for Limits {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_limits_defaults_are_fail_closed() {
        let l = Limits::new();
        assert_eq!(l.max_output(), 64 << 20);
        assert_eq!(l.max_ratio(), 0);
        assert_eq!(l.ratio_floor(), 1 << 20);
        assert_eq!(l.max_header(), 64 << 10);
        assert_eq!(l.max_members(), 1024);
        assert_eq!(Limits::default(), l);
    }

    #[test]
    fn test_limits_ratio_off_never_exceeded() {
        assert!(!Limits::new().ratio_exceeded(u64::MAX, 1));
    }

    #[test]
    fn test_limits_ratio_below_floor_not_checked() {
        let l = Limits::new().with_max_ratio(2).with_ratio_floor(100);
        assert!(!l.ratio_exceeded(100, 1));
        assert!(l.ratio_exceeded(101, 1));
        assert!(!l.ratio_exceeded(101, 51));
        assert!(l.ratio_exceeded(103, 51));
    }

    #[test]
    fn test_limits_ratio_saturates_on_huge_input() {
        let l = Limits::new().with_max_ratio(u32::MAX).with_ratio_floor(0);
        assert!(!l.ratio_exceeded(u64::MAX, u64::MAX));
    }

    #[test]
    fn test_limits_unlimited_has_no_output_cap() {
        let l = Limits::unlimited();
        assert_eq!(l.max_output(), u64::MAX);
        assert_eq!(l.max_members(), u32::MAX);
        assert_eq!(l.with_max_members(3).max_members(), 3);
    }
}
