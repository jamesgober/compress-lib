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
//! ## Status
//!
//! `0.1.0`, the scaffold: the crate claims its name and carries the plan.
//! The codecs arrive from `0.3` (inflate) and `0.4` (deflate and RFC 7692);
//! the complete planned API, each item with its version, is `dev/PLAN.md` in
//! the [repository](https://github.com/jamesgober/compress-lib).

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

/// Crate version string, populated by Cargo at build time.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

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
