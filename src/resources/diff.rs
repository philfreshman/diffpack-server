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
//! # Why nothing here walks a tree, or builds one
//!
//! The totals are `diff_package_versions`'s, the nodes are `get_diff_tree`'s
//! and the comparison itself is `diff_package_versions::compare`'s. All three
//! are *called* rather than reproduced. A resource that walked the tree
//! itself would be a second answer to a question a tool already answers, and
//! the two would disagree the first time either changed — the drift ADR 0013
//! records for the patch renderer, in a second place. What this module owns
//! is the document the three go into.
//!
//! The third of them used to be here: this module held the fetch, the
//! extraction and the tree build, and three tools had a copy of the same
//! walk. That was a resource computing what a tool computes, which is what
//! [ADR 0014](../../docs/adr/0014-a-resource-is-a-projection-of-the-tools.md)
//! forbids and what [ADR
//! 0016](../../docs/adr/0016-the-walk-to-a-comparison-is-this-tools.md) moved
//! to the tool that owns it. Neither `futures` nor `crate::engine` is
//! imported here any more, which is the short version of the same sentence.

use rmcp::model::{CacheScope, ReadResourceResult, Resource, ResourceContents, ResourceTemplate};
use serde::Serialize;

use crate::error::Failure;
use crate::handle::{DiffHandle, Inputs};
use crate::page;
use crate::tools::get_diff_tree::{self, Node};
use crate::tools::{diff_package_versions, Call, Tool};

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

/// The URI one comparison is read at.
pub fn uri_of(handle: &DiffHandle) -> String {
    format!("{PREFIX}{}", handle.encode())
}

/// A link to the comparison `handle` names, for a tool's answer to carry.
///
/// Built here rather than by the tool that mints the handle, so that the URI
/// format stays this module's: a tool that spelled it out would be the second
/// place `diffpack://diff/` is written, and the first to be wrong when it
/// moves.
pub fn link(handle: &DiffHandle) -> Resource {
    Resource::new(uri_of(handle), "diff")
        .with_title("This comparison")
        .with_description(
            "The whole of the comparison this call made: what was compared, how much \
             changed, and every file and directory in it. Reading it is the same answer \
             `get_diff_tree` pages through.",
        )
        .with_mime_type("application/json")
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

/// One comparison, whole — or, when it does not fit, everything known about
/// it and where to read the rest.
pub async fn read(handle: &DiffHandle, call: &Call) -> Result<ReadResourceResult, Failure> {
    let tree = diff_package_versions::compare(handle, call).await?.tree;
    let totals = diff_package_versions::totals(&tree);

    let whole = write(&Document {
        inputs: handle.inputs(),
        totals: &totals,
        tree: Some(get_diff_tree::nodes(&tree)),
        tree_too_large: None,
    })?;

    // Measured after it is built rather than guessed at from a node count,
    // for the reason `page` measures a page the same way: the difference
    // between a guess and the answer is a package whose paths are long, and
    // this is exactly such a package.
    let text = if page::fits(&whole) {
        whole
    } else {
        write(&Document {
            inputs: handle.inputs(),
            totals: &totals,
            // Absent, not cut. A tree is whole or it is misleading: the first
            // nine tenths of one reads exactly like all of it, and an agent
            // looking for a file in the last tenth is told it is not there.
            tree: None,
            tree_too_large: Some(TooLarge::at(whole.len())),
        })?
    };

    let contents = ResourceContents::text(text, uri_of(handle)).with_mime_type("application/json");

    Ok(ReadResourceResult::new(vec![contents])
        .with_ttl_ms(TTL_MS)
        .with_cache_scope(CacheScope::Public))
}

/// What a reader is handed.
///
/// Exactly one of `tree` and `tree_too_large` is there. Both are skipped when
/// absent rather than written as `null`, so a reader that finds no `tree` and
/// no statement about one has been handed something this module did not
/// build.
#[derive(Serialize)]
struct Document<'d> {
    /// What was compared. A URI carries an opaque handle and a document read
    /// out of a resource browser has no call beside it saying what was asked
    /// for, so without this the totals are a comparison of something.
    inputs: &'d Inputs,

    /// How much changed, in files and in lines. Always here — it is a handful
    /// of numbers however large the comparison is, and it is most of what a
    /// reader wanted.
    totals: &'d diff_package_versions::Totals,

    /// Every file and directory in the comparison, in the order
    /// `get_diff_tree` walks them.
    #[serde(skip_serializing_if = "Option::is_none")]
    tree: Option<Vec<Node>>,

    /// Why the tree is not here, when it is not.
    #[serde(skip_serializing_if = "Option::is_none")]
    tree_too_large: Option<TooLarge>,
}

/// What stands in for a tree that does not fit in one answer.
///
/// A statement about the tree rather than a piece of it: how big it came to,
/// the ceiling it was measured against, and the tool that walks the same tree
/// a page at a time. The numbers are here as well as the sentence because a
/// reader that has to parse prose to learn it got less than everything will
/// eventually not parse it.
#[derive(Serialize)]
struct TooLarge {
    /// How many bytes this comparison came to with its tree in it.
    bytes: usize,

    /// The most one answer can carry.
    ceiling: usize,

    /// The tool that walks the same tree, a page at a time.
    read_with: &'static str,

    /// The same thing in a sentence, for a reader that has only this
    /// document.
    note: String,
}

impl TooLarge {
    fn at(bytes: usize) -> Self {
        Self {
            bytes,
            ceiling: page::PAYLOAD_CEILING,
            read_with: TREE_TOOL,
            note: format!(
                "This comparison's tree is {bytes} bytes and one answer carries at most \
                 {}, so it is not in this document — none of it, rather than as much as \
                 fits, because part of a tree reads exactly like all of one. Call \
                 `{TREE_TOOL}` with the same handle to walk it a page at a time; the \
                 totals above are the whole comparison's either way.",
                page::PAYLOAD_CEILING,
            ),
        }
    }
}

/// The tool that pages through what this document could not carry.
///
/// Its own name, taken from the tool rather than written out, so that a tool
/// renamed is not a resource pointing at a call that does not exist.
const TREE_TOOL: &str = <get_diff_tree::GetDiffTree as Tool>::NAME;

/// `document`, as the text a read answers with.
fn write(document: &Document<'_>) -> Result<String, Failure> {
    serde_json::to_string_pretty(document).map_err(|_| Failure::Internal {
        doing: "answering a resource read",
    })
}
