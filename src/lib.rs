//! # compress_lib
//!
//! DEFLATE (RFC 1951), zlib (RFC 1950), and gzip (RFC 1952) for Rust
//! services, with the per-message rules of WebSocket permessage-deflate
//! (RFC 7692).
//!
//! The engine is sans-IO: the caller pushes input and pulls output through
//! buffers it owns. Memory is fixed when an engine is built and never grows
//! with input, and every decoder enforces an absolute output cap while it
//! decodes, so a decompression bomb costs at most the cap. There is no
//! `unsafe` code and no third-party runtime dependency.
//!
//! ## Tier 1
//!
//! Add it with Cargo's rename, `compress = { package = "compress-lib", version
//! = "0.2" }`, and code reads `compress::`. In 0.2 the checksums are ready:
//!
//! ```
//! use compress_lib::checksum::{adler32, crc32};
//!
//! assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
//! assert_eq!(adler32(b"Wikipedia"), 0x11E6_0398);
//! ```
//!
//! Decoders take [`Limits`] (64 MiB of output by default) and encoders a
//! [`Level`] (0 to 9, zlib's meaning). Every failure is an [`Error`] with a
//! matchable [`ErrorKind`] and a stable code for logs and metrics.
//!
//! ## Status
//!
//! `0.2.0`, the design lock: the complete planned API, each item with the
//! version it arrives in, is `docs/API.md` in the
//! [repository](https://github.com/jamesgober/compress-lib). The checksums,
//! the error type, the limits, and the levels ship now; the raw DEFLATE
//! inflater arrives in `0.3`, the deflater and the RFC 7692 helpers in `0.4`,
//! zlib and gzip in `0.5`.

#![doc(html_root_url = "https://docs.rs/compress-lib")]
#![cfg_attr(docsrs, feature(doc_cfg))]
#![cfg_attr(not(feature = "std"), no_std)]
#![forbid(unsafe_code)]
#![deny(missing_docs)]
#![cfg_attr(
    not(test),
    deny(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::todo,
        clippy::unimplemented,
        clippy::unreachable,
        clippy::dbg_macro
    )
)]

extern crate alloc;

/// Crate version string, populated by Cargo at build time.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

pub mod checksum;

mod error;
mod level;
mod limits;

pub use error::{Error, ErrorKind};
pub use level::Level;
pub use limits::Limits;

#[cfg(test)]
mod tests {
    use super::VERSION;

    #[test]
    fn test_version_matches_manifest() {
        assert_eq!(VERSION, env!("CARGO_PKG_VERSION"));
        let core = VERSION.split(['-', '+']).next().unwrap_or_default();
        assert_eq!(core.split('.').count(), 3);
        assert!(core.split('.').all(|part| part.parse::<u64>().is_ok()));
    }
}
