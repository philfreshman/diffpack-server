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
//! # What is written here rather than re-exported
//!
//! [`build_diff_tree`], which is the engine's own but takes two [`FileMap`]s
//! rather than the maps inside them. A FileMap is the archive seam's type,
//! with the extractor's map private to it (#95), and this is the one place
//! that map is handed back to the engine — the crossing belongs in the module
//! that names the engine, and a caller building a tree hands over FileMaps
//! rather than reading the entries inside them.
//!
//! [`build_patch`], the four-case renderer for one file, used to be written
//! out here too: until `0.4.0` the engine kept it private to its
//! `wasm_bindgen` layer. It is re-exported now, with the [`Patch`] it returns,
//! so the server and the browser render a file through one function rather
//! than a function and a transcription of it (ADR 0013). A `Patch` is also
//! what `patches.json` stores, and the engine's serialises as `data` and
//! `is_diff`, the names the one written here did: `tests/store.rs` pins that
//! shape, so a release that renamed a field fails there rather than leaving
//! every stored Entry unreadable.
//!
//! # One lookup per registry
//!
//! [`archive_source`], [`choose_archive`] and [`unpack_archive`] are the
//! engine's per-registry answers to where a version's archive is, which of a
//! listing's files it is, and how to unpack it. None of them fetches:
//! [`crate::archive`] does, with this server's client, size cap and
//! failures, between the steps. They replace the URL builder, the sdist
//! chooser and the extractor this module used to re-export one by one, so a
//! registry's archive is described once, upstream, for both callers.
//!
//! # What is deliberately not re-exported
//!
//! The Go helpers (`build_go_zip_url`, `escape_go_module_path`,
//! `escape_go_version`, `strip_go_module_root`). Nothing here calls them, and
//! #28 should not need to: `archive_source` and `unpack_archive` answer for
//! `go` like any other registry, including the case-escaping of the version
//! and the `<module>@<version>/` prefix, which `extract_archive_bytes` alone
//! gets wrong. A re-export with no caller is a surface we would have to keep
//! working.

use crate::archive::FileMap;

pub use diffpack_engine::{
    archive_source, build_patch, choose_archive, get_diff_content, unpack_archive, whitespace_mode,
    ArchiveSource, DiffFileEntry, DiffStatus, FileMapEntry, FileType, Patch, WhitespaceMode,
};

/// The tree of what changed between two versions' files.
///
/// `diffpack-engine`'s `build_diff_tree`, handed the maps inside two
/// [`FileMap`]s. Written out rather than re-exported for the reason the
/// module header gives: the engine takes the map a FileMap keeps private.
pub fn build_diff_tree(
    from: &FileMap,
    to: &FileMap,
    similarity_threshold: f64,
    ignore_whitespace: bool,
) -> DiffFileEntry {
    diffpack_engine::build_diff_tree(
        from.as_engine_map(),
        to.as_engine_map(),
        similarity_threshold,
        ignore_whitespace,
    )
}

/// The pinned `diffpack-engine` release.
///
/// A field in the cache key, not a label: see
/// [`crate::cache_key::DiffKey::engine`]. `tests/engine.rs` fails if this and
/// the tag in `Cargo.toml` disagree.
pub const VERSION: &str = "0.4.0";
