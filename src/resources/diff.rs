//! `diffpack://diff/{handle}` — a whole comparison, read rather than called.
//!
//! What `get_diff_tree` answers a page at a time, in one document: the
//! inputs, the totals and the tree. The segment is the handle
//! [`crate::handle`] mints and not a `diff_id` — a `diff_id` is a hash, so a
//! URI carrying one could only ever be read out of the cache and would stop
//! resolving the moment an entry was evicted. With the handle the read
//! recomputes instead, which is the same reason `get_diff_tree` takes one.
//! See [ADR 0006](../../docs/adr/0006-the-handle-carries-its-inputs.md).
//!
//! # Why nothing here walks a tree
//!
//! The totals are `diff_package_versions`'s and the nodes are
//! `get_diff_tree`'s, and both are *called* rather than reproduced. A
//! resource that walked the tree itself would be a second answer to a
//! question a tool already answers, and the two would disagree the first time
//! either changed — the drift ADR 0013 records for the patch renderer, in a
//! second place. What this module owns is the document the three go into.

use futures::try_join;
use rmcp::model::{CacheScope, ReadResourceResult, ResourceContents, ResourceTemplate};
use serde::Serialize;

use crate::archive::FileMap;
use crate::engine::{self, DiffFileEntry};
use crate::error::Failure;
use crate::handle::{DiffHandle, Inputs};
use crate::tools::get_diff_tree::{self, Node};
use crate::tools::{diff_package_versions, Ctx};

/// Everything before the handle. The template below is built from it, so
/// there is one spelling of this prefix and the matcher and the template
/// cannot describe different URIs.
pub const PREFIX: &str = "diffpack://diff/";

/// The URI a client fills in to read one comparison.
pub fn uri_template() -> String {
    format!("{PREFIX}{{handle}}")
}

/// The handle `uri` names, if this is a URI of ours at all.
///
/// One segment and no more, which is what tells this apart from the file
/// diff's URI without either matcher having to be tried first: a handle is a
/// version prefix and base64url, and neither carries a `/`.
pub fn handle_in(uri: &str) -> Option<&str> {
    let handle = uri.strip_prefix(PREFIX)?;
    (!handle.is_empty() && !handle.contains('/')).then_some(handle)
}

/// The template, as `resources/templates/list` shows it.
pub fn template() -> ResourceTemplate {
    ResourceTemplate::new(uri_template(), "diff")
        .with_title("Package comparison")
        .with_description(
            "One comparison of two published versions: what was compared, how much \
             changed, and every file and directory in it. `{handle}` is the handle \
             `diff_package_versions` gave you, passed back exactly as it arrived. A \
             comparison too large to serve whole comes back as its totals with a pointer \
             to `get_diff_tree`, which pages through the same tree.",
        )
        .with_mime_type("application/json")
}

/// How long a client may treat one comparison as fresh.
///
/// A day, where the catalogue gets an hour, because this answer cannot
/// change: a handle names two published versions, which are immutable, and it
/// carries the engine and the schema it was minted under — so a build that
/// would compare them differently is a build that refuses the handle outright
/// rather than answering it another way.
const TTL_MS: u64 = 24 * 60 * 60 * 1000;

/// One comparison: both versions' files, and the tree they compare to.
///
/// What every read under `diffpack://diff/` starts with, including the one
/// file's diff next door — which needs the tree to find where a renamed file
/// was, and both file maps to render it. Made once here so that reading one
/// file costs one pair of downloads rather than two.
pub struct Comparison {
    pub from_files: FileMap,
    pub to_files: FileMap,
    pub tree: DiffFileEntry,
}

/// Fetch both versions of what `handle` names and compare them.
pub async fn compare(handle: &DiffHandle, ctx: &Ctx) -> Result<Comparison, Failure> {
    let inputs = handle.inputs();

    // Concurrently, the way the tool that mints a handle fetches them: the
    // two downloads do not depend on each other.
    let (from_files, to_files) = try_join!(
        ctx.archive()
            .fetch(inputs.registry, &inputs.package, &inputs.from_version),
        ctx.archive()
            .fetch(inputs.registry, &inputs.package, &inputs.to_version),
    )?;

    let tree = engine::build_diff_tree(
        &from_files,
        &to_files,
        inputs.similarity_threshold,
        inputs.ignore_whitespace,
    );

    Ok(Comparison {
        from_files,
        to_files,
        tree,
    })
}

/// One comparison, whole.
pub async fn read(handle: &DiffHandle, ctx: &Ctx) -> Result<ReadResourceResult, Failure> {
    let tree = compare(handle, ctx).await?.tree;

    let document = Document {
        inputs: handle.inputs(),
        totals: diff_package_versions::totals(&tree),
        tree: get_diff_tree::nodes(&tree),
    };

    let contents = ResourceContents::text(
        serde_json::to_string_pretty(&document).map_err(|_| Failure::Internal {
            doing: "answering a resource read",
        })?,
        format!("{PREFIX}{}", handle.encode()),
    )
    .with_mime_type("application/json");

    Ok(ReadResourceResult::new(vec![contents])
        .with_ttl_ms(TTL_MS)
        .with_cache_scope(CacheScope::Public))
}

/// What a reader is handed.
#[derive(Serialize)]
struct Document<'d> {
    /// What was compared. A URI carries an opaque handle and a document read
    /// out of a resource browser has no call beside it saying what was asked
    /// for, so without this the totals are a comparison of something.
    inputs: &'d Inputs,

    /// How much changed, in files and in lines.
    totals: diff_package_versions::Totals,

    /// Every file and directory in the comparison, in the order
    /// `get_diff_tree` walks them.
    tree: Vec<Node>,
}
