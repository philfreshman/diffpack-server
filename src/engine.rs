//! The one place this crate names `diffpack-engine`.
//!
//! The server computes diffs with the same code the browser runs, as a native
//! Cargo dependency rather than through wasm. Re-implementing any of it here
//! would mean two copies of an output format that has to stay byte-identical,
//! with nothing keeping them honest.
//!
//! Everything else in the crate goes through this module, and
//! `scripts/check-engine-seam.sh` fails the build if anything else imports
//! `diffpack_engine` directly. One seam means one file to change when the
//! engine moves, and one file to read to know what we depend on.
//!
//! # What is not here yet
//!
//! `get_diff_content` (the unified-diff renderer), `whitespace_mode` and the
//! npm/crates.io/PyPI URL builders live in the engine's private `core` and
//! `package` modules and are unreachable from outside it. philfreshman/diffpack-engine#2
//! widens that surface and releases `0.3.0`; this module grows to cover them
//! when it lands, and [`VERSION`] moves with the tag in `Cargo.toml`.

pub use diffpack_engine::{
    build_diff_tree, extract_archive_bytes, DiffFileEntry, DiffStatus, FileMapEntry, FileType,
};

/// The pinned `diffpack-engine` release.
///
/// A field in the cache key, not a label: see
/// [`crate::cache_key::DiffKey::engine`]. `tests/engine.rs` fails if this and
/// the tag in `Cargo.toml` disagree.
pub const VERSION: &str = "0.2.0";
