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
//! # What is deliberately not re-exported
//!
//! The Go helpers (`build_go_zip_url`, `escape_go_module_path`,
//! `strip_go_module_root`). The engine made them public in `0.3.0` so that Go
//! support would not need a second release, but nothing here calls them yet
//! and a re-export with no caller is a surface we would have to keep working.
//! #28 adds them.

pub use diffpack_engine::{
    build_diff_tree, build_tarball_url, extract_archive_bytes, get_diff_content,
    select_pypi_sdist_url, whitespace_mode, DiffFileEntry, DiffStatus, FileMapEntry, FileType,
    PyPiResponse, PyPiUrl, WhitespaceMode,
};

/// The pinned `diffpack-engine` release.
///
/// A field in the cache key, not a label: see
/// [`crate::cache_key::DiffKey::engine`]. `tests/engine.rs` fails if this and
/// the tag in `Cargo.toml` disagree.
pub const VERSION: &str = "0.3.0";
