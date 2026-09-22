//! `diffpack://diff/{handle}/file/{path}` — one file's diff out of a
//! comparison.
//!
//! The same bytes [`get_file_diff`](crate::tools::get_file_diff) returns at
//! its defaults, which is three lines of unchanged context around each change
//! — the settings a caller gets by passing nothing but a handle and a path,
//! which is all a URI has room for. The tool is *called* rather than
//! reproduced, so the two cannot render one file two ways.
//!
//! # What the media type carries
//!
//! What the tool says in `isDiff`, this says in the one field a client
//! already reads to decide how to render something: a patch is `text/x-diff`
//! and the two answers that are not patches — a file both versions ship byte
//! for byte, and a path neither version has — are `text/plain`. A reader that
//! had to check a field first would eventually parse `@@` out of a file that
//! has none.
//!
//! # Why the old path is looked up rather than asked for
//!
//! A renamed file needs both of its paths, and the tool's own description
//! tells a caller to pass the `old_path` the tree gives it. A URI has room
//! for one path, so this looks the other up in the comparison rather than
//! leaving it out — the alternative being an answer that reports every line
//! of a moved file as added, which is wrong about a file the package still
//! ships and which a reader has nothing in the answer to doubt with. It is
//! the same departure `get_file_diff` makes for a directory, and for the same
//! reason.

use rmcp::model::{CacheScope, ReadResourceResult, ResourceContents, ResourceTemplate};

use crate::error::Failure;
use crate::handle::DiffHandle;
use crate::tools::get_diff_tree;
use crate::tools::get_file_diff::{self, Args};
use crate::tools::Ctx;

/// What stands between the handle and the path.
const SEPARATOR: &str = "/file/";

/// The URI a client fills in to read one file's diff.
pub fn uri_template() -> String {
    format!("{}{{handle}}{SEPARATOR}{{path}}", super::diff::PREFIX)
}

/// The handle and the path `uri` names, if this is a URI of ours at all.
///
/// The path is taken as it stands, separators and all: it is the last thing
/// in the URI, so there is nothing after it to be confused with. A handle
/// carries no `/`, which is what tells this apart from the whole
/// comparison's URI without either matcher having to be tried first.
pub fn parts_in(uri: &str) -> Option<(&str, &str)> {
    let rest = uri.strip_prefix(super::diff::PREFIX)?;
    let (handle, path) = rest.split_once(SEPARATOR)?;

    (!handle.is_empty() && !handle.contains('/') && !path.is_empty()).then_some((handle, path))
}

/// The template, as `resources/templates/list` shows it.
pub fn template() -> ResourceTemplate {
    ResourceTemplate::new(uri_template(), "file-diff")
        .with_title("File diff")
        .with_description(
            "One file's diff out of a comparison, with three lines of unchanged context \
             around each change. `{handle}` is the handle `diff_package_versions` gave \
             you and `{path}` is a path from that comparison's tree, with the archive's \
             top-level directory already removed. A `text/x-diff` answer is a patch; a \
             `text/plain` one is not — a file both versions ship unchanged comes back as \
             itself, and a path neither version has comes back as a sentence saying so.",
        )
        .with_mime_type("text/x-diff")
}

/// How long a client may treat one file's diff as fresh.
///
/// A day, for the reason the whole comparison gets one: the handle names two
/// published versions and the build that compared them, so this answer cannot
/// change without the handle being refused outright.
const TTL_MS: u64 = 24 * 60 * 60 * 1000;

/// One file of one comparison.
pub async fn read(
    handle: &DiffHandle,
    path: &str,
    ctx: &Ctx,
) -> Result<ReadResourceResult, Failure> {
    let comparison = super::diff::compare(handle, ctx).await?;

    let patch = get_file_diff::render(
        &comparison.from_files,
        &comparison.to_files,
        handle.inputs(),
        &Args {
            handle: handle.clone(),
            path: path.to_owned(),
            old_path: moved_from(&comparison, path),
            // The defaults a caller gets by passing nothing but a handle and
            // a path, which is all a URI has room for.
            context_lines: get_file_diff::ContextLines::default(),
            max_bytes: None,
        },
    )?;

    let contents = ResourceContents::text(
        patch.excerpt.text,
        format!("{}{SEPARATOR}{path}", super::diff::uri_of(handle)),
    )
    .with_mime_type(if patch.is_diff {
        "text/x-diff"
    } else {
        "text/plain"
    });

    Ok(ReadResourceResult::new(vec![contents])
        .with_ttl_ms(TTL_MS)
        .with_cache_scope(CacheScope::Public))
}

/// Where the file at `path` was in the first version, if it moved.
///
/// The comparison's own answer, which is the one the tool's description tells
/// a caller to pass. Nothing for every other file, which is what the tool
/// wants for them — and nothing for a path the comparison does not have,
/// which the renderer answers with the sentence saying so.
///
/// A descent rather than a walk, which is `get_diff_tree`'s and not this
/// module's: one step per directory rather than a scan of everything above
/// the file.
fn moved_from(comparison: &super::diff::Comparison, path: &str) -> Option<String> {
    get_diff_tree::node_at(&comparison.tree, path)?
        .old_path
        .clone()
}
