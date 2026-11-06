//! CRC-32 and Adler-32.
//!
//! The checksums gzip (CRC-32) and zlib (Adler-32) carry. The framing
//! decoders verify them automatically; they are public because they are
//! useful on their own (file integrity, cache keys, zip entries).
//!
//! ```
//! use compress_lib::checksum::{adler32, crc32};
//!
//! assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
//! assert_eq!(adler32(b"Wikipedia"), 0x11E6_0398);
//! ```

mod adler32;
mod crc32;

pub use adler32::{Adler32, adler32, adler32_combine};
pub use crc32::{Crc32, crc32, crc32_combine};
