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
//! = "1" }`, and code reads `compress::`. One call each way; decoding is
//! capped at 64 MiB of output by default:
//!
//! ```
//! use compress_lib as compress;
//!
//! let packed = compress::gzip::compress(b"hello, hello, hello");
//! let body = compress::gzip::decompress(&packed)?;
//! assert_eq!(body, b"hello, hello, hello");
//!
//! let z = compress::zlib::compress(b"zlib, which HTTP calls deflate");
//! let raw = compress::deflate::compress_with(b"raw DEFLATE", compress::Level::FAST);
//! assert_eq!(compress::decompress(&z)?, b"zlib, which HTTP calls deflate"); // gzip or zlib, detected
//!
//! let small = compress::Limits::new().with_max_output(1 << 20);
//! assert_eq!(compress::deflate::decompress_with(&raw, small)?, b"raw DEFLATE");
//! # Ok::<(), compress_lib::Error>(())
//! ```
//!
//! ## WebSocket permessage-deflate
//!
//! [`message`] has the RFC 7692 rules: the sync-flush tail removed and put
//! back, context takeover on or off, window bits 8 to 15, and the output
//! budget checked while inflating.
//!
//! ```
//! use compress_lib::message::{Compressor, Decompressor, MessageConfig};
//!
//! let config = MessageConfig::new().with_window_bits(10);
//! let mut tx = Compressor::new(config)?;
//! let mut rx = Decompressor::new(config)?;
//! let mut wire = Vec::new();
//! tx.compress_message(b"{\"op\":\"ping\"}", &mut wire)?;
//! let mut text = Vec::new();
//! rx.decompress_message(&wire, &mut text, 1 << 20)?;
//! assert_eq!(text, b"{\"op\":\"ping\"}");
//! # Ok::<(), compress_lib::Error>(())
//! ```
//!
//! ## Streaming
//!
//! [`deflate::Inflater`] and [`deflate::Deflater`] take buffers you own and
//! say where they stopped ([`Status`]); memory is fixed when they are built,
//! and every decoder limit is checked before output is written.
//!
//! ```
//! use compress_lib::deflate::{Deflater, Flush, Inflater, Status};
//!
//! let mut deflater = Deflater::default();
//! let mut packed = [0u8; 256];
//! let p = deflater.compress(b"streamed", &mut packed, Flush::Finish)?;
//! assert_eq!(p.status, Status::Done);
//!
//! let mut inflater = Inflater::new().with_window_bits(12)?;
//! let mut out = [0u8; 64];
//! let q = inflater.finish(&packed[..p.produced], &mut out)?;
//! assert_eq!(&out[..q.produced], b"streamed");
//! # Ok::<(), compress_lib::Error>(())
//! ```
//!
//! [`Decoder`] and [`Encoder`] take a [`Format`] chosen at run time (an HTTP
//! layer picking by `Content-Encoding`); with the `std` feature, [`io`] wraps
//! any of them around `Read` and `Write`.
//!
//! Every failure is an [`Error`] with a matchable [`ErrorKind`] and a stable
//! code for logs and metrics. [`checksum`] has CRC-32 and Adler-32.
//!
//! ## Status
//!
//! `1.1.0`, stable: no breaking change to the public API before 2.0. What
//! 1.x promises and the audit behind it are in `docs/STABILITY-1.0.md` in
//! the [repository](https://github.com/jamesgober/compress-lib), with
//! `docs/API.md`, `docs/ARCHITECTURE.md`, `docs/PERFORMANCE.md`, and
//! `docs/SECURITY.md` beside it.

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
pub mod deflate;
pub mod gzip;
#[cfg(feature = "std")]
#[cfg_attr(docsrs, doc(cfg(feature = "std")))]
pub mod io;
pub mod message;
pub mod zlib;

mod error;
mod format;
mod inflate;
mod level;
mod limits;
mod stream;

pub use deflate::Flush;
pub use error::{Error, ErrorKind};
pub use format::{Decoder, Encoder, Format, decompress, decompress_with};
pub use level::Level;
pub use limits::Limits;
pub use stream::{Progress, Status};

/// Ends a one-shot compression. The encoders fail only when the output
/// vector cannot grow, and a one-shot call has no error to return, so that
/// aborts as growing a `Vec` does rather than hand back a truncated stream
/// (D-059).
pub(crate) fn one_shot_or_abort(result: Result<Progress, Error>, out: &alloc::vec::Vec<u8>) {
    if result.is_err() {
        let wanted = out.capacity().saturating_mul(2).max(1);
        let layout =
            core::alloc::Layout::array::<u8>(wanted).unwrap_or(core::alloc::Layout::new::<u8>());
        alloc::alloc::handle_alloc_error(layout);
    }
}

/// Overwrites every element of `v` with the default value, spare capacity
/// included, keeping its length, and passes it through
/// [`core::hint::black_box`] so the writes are not removed as dead stores.
/// Without `unsafe` (volatile writes) this is a strong hint to the compiler,
/// not a guarantee; the documentation of every `wipe` says so.
pub(crate) fn wipe_vec<T: Copy + Default>(v: &mut alloc::vec::Vec<T>) {
    let len = v.len();
    v.resize(v.capacity(), T::default());
    v.fill(T::default());
    core::hint::black_box(&*v);
    v.truncate(len);
}

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
