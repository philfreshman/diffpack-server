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
//! [`patch`], the four-case renderer for one file. The engine has it, as
//! `build_diff_result`, but private to its `wasm_bindgen` layer: it is not
//! part of the crate's native surface and so cannot be re-exported. Writing
//! it here is the cost of that, and this is the right place to pay it — one
//! rendering, in the module that names the engine version it is pinned to,
//! rather than one in the tool that caches a result and another in the tool
//! that renders one on demand.
//!
//! [`build_diff_tree`], which is the engine's own but takes two [`FileMap`]s
//! rather than the maps inside them. A FileMap is the archive seam's type,
//! with the extractor's map private to it (#95), and this is the one place
//! that map is handed back to the engine — the crossing belongs in the module
//! that names the engine, and a caller building a tree hands over FileMaps
//! rather than reading the entries inside them.
//!
//! # What is deliberately not re-exported
//!
//! The Go helpers (`build_go_zip_url`, `escape_go_module_path`,
//! `strip_go_module_root`). The engine made them public in `0.3.0` so that Go
//! support would not need a second release, but nothing here calls them yet
//! and a re-export with no caller is a surface we would have to keep working.
//! #28 adds them.

use serde::{Deserialize, Serialize};

use crate::archive::FileMap;

pub use diffpack_engine::{
    build_tarball_url, extract_archive_bytes, get_diff_content, select_pypi_sdist_url,
    whitespace_mode, DiffFileEntry, DiffStatus, FileMapEntry, FileType, PyPiResponse, PyPiUrl,
    WhitespaceMode,
};

/// One file's rendered diff.
///
/// `is_diff` says whether `data` is a diff at all. A file whose content did
/// not change, and one that is in neither version, are not diffs — so a
/// reader renders them as a file and as a sentence rather than as a diff of
/// all-context lines.
///
/// The field names are the ones `docs/architecture.md` and #21 write down,
/// which are not the ones the engine's browser binding uses: that one is
/// `camelCase` over the wire to JavaScript, and this is a blob in a store
/// whose every other field is `snake_case`. The two never meet — nothing
/// reads a `patches.json` through wasm — and a blob that spelled one field
/// the other way would be the only one that did.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Patch {
    pub data: String,
    pub is_diff: bool,
}

/// The patch for `filename`, given what each version had of it.
///
/// This is `diffpack-engine`'s `build_diff_result`, which is private to the
/// crate's `wasm_bindgen` layer and so cannot be re-exported the way
/// everything else here is. It is written out rather than left to each
/// caller because there are two — the cache renders every changed file at
/// the moment both archives are extracted (#21), and `get_file_diff` (#15)
/// renders one on demand, by which time it costs two downloads — and two
/// renderings of one file that disagree is exactly the drift this module
/// exists to prevent.
///
/// Only the fourth case is a diff the engine computes. The other three are
/// the engine's presentation of a file that one side does not have, which is
/// why they can be written here at all: a line prefix and a header, with
/// nothing to get subtly wrong that a test cannot see.
pub fn patch(
    filename: &str,
    from: Option<&str>,
    to: Option<&str>,
    ignore_whitespace: bool,
) -> Patch {
    match (from, to) {
        (None, None) => Patch {
            data: "File not present in either version.".to_owned(),
            is_diff: false,
        },

        (None, Some(to)) => Patch {
            data: sided(&format!("--- /dev/null\n+++ to/{filename}"), '+', to),
            is_diff: true,
        },

        (Some(from), None) => Patch {
            data: sided(&format!("--- from/{filename}\n+++ /dev/null"), '-', from),
            is_diff: true,
        },

        // Identical content is the file rather than a diff of it, and it is
        // how a rename that moved a file without touching it reads.
        //
        // Byte equality, and deliberately not the comparison
        // `ignore_whitespace` would make: a file that was reformatted *did*
        // change, and answering with its content would hide the reformatting
        // that is the only thing that happened to it. The engine draws the
        // line here too.
        (Some(from), Some(to)) if from == to => Patch {
            data: to.to_owned(),
            is_diff: false,
        },

        (Some(from), Some(to)) => Patch {
            data: get_diff_content(filename, from, to, ignore_whitespace),
            is_diff: true,
        },
    }
}

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

/// `header`, then every line of `content` behind `sign`.
///
/// Split on `\n` and not by lines, so a trailing newline leaves a last empty
/// line the way the engine's does. That is the whole of the difference
/// between this and the obvious version of it.
fn sided(header: &str, sign: char, content: &str) -> String {
    let mut rendered = String::from(header);
    for line in content.split('\n') {
        rendered.push('\n');
        rendered.push(sign);
        rendered.push(' ');
        rendered.push_str(line);
    }
    rendered
}

/// The pinned `diffpack-engine` release.
///
/// A field in the cache key, not a label: see
/// [`crate::cache_key::DiffKey::engine`]. `tests/engine.rs` fails if this and
/// the tag in `Cargo.toml` disagree.
pub const VERSION: &str = "0.4.0";
