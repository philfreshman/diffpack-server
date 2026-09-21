//! `get_diff_tree` — the tree `diff_package_versions` answered without.
//!
//! That tool returns totals and a handle because a large package's tree does
//! not fit under the response ceiling and would not be worth an agent's
//! context if it did. This one returns the tree itself, in the slices an
//! agent actually wants: one subtree, one set of statuses, one page at a
//! time.
//!
//! # Why a flat sequence and not a nested one
//!
//! The answer is a page of nodes, each carrying its whole path, rather than
//! the engine's nested shape. A sequence is what can be cut at an arbitrary
//! point and resumed, which is the whole of what pagination is; a nested
//! answer would need a cursor naming a path and an index inside it, which is
//! a second cursor format. See [ADR
//! 0012](../../docs/adr/0012-a-tree-is-paged-as-a-flat-sequence.md).
//!
//! # What the listing is rooted at
//!
//! A directory is not inside its own subtree, so neither the comparison's
//! root nor a directory named by `path` is in its own listing — the same rule
//! [`super::list_package_files`]'s `prefix` follows, and for the same reason:
//! an agent that asked what is under `src` is not asking about `src`.
//!
//! # Where a cached result would come in
//!
//! Nowhere yet, and that is worth saying because it looks like an omission.
//! The DiffStore arrives with #21; until it does, every call recomputes the
//! comparison from the inputs the handle carries. That is precisely the path
//! a cache miss takes, so the behaviour #44 exists for — an evicted entry is
//! recomputed and served rather than refused — is the only behaviour there
//! is here, and the tests that hold it will go on holding it once a store is
//! in front of it.
//!
//! # Where the descriptions come from
//!
//! Every doc comment on a field of [`Args`], [`Node`], [`NodeType`] and
//! [`Status`] becomes a `description` in a schema a model reads, so it is
//! written for that reader and names nothing in this repository. Why a field
//! is shaped the way it is belongs here or in an ordinary comment beside the
//! code.
//!
//! Three fields have no doc comment at all, deliberately. `handle` is
//! [`crate::handle`]'s type and `cursor` and `limit` are [`crate::page`]'s,
//! and those modules write their descriptions — where a handle comes from
//! and that it is not built by hand, the default and the range of a limit,
//! the rule that a cursor is passed back unchanged. A doc comment here would
//! *replace* those rather than add to them, which is how four tools that
//! take one handle end up describing it four ways.

use futures::try_join;
use serde::{Deserialize, Serialize};

use crate::engine::{self, DiffFileEntry, DiffStatus, FileType};
use crate::error::Failure;
use crate::handle::DiffHandle;
use crate::page::{self, Page};
use crate::tools::{Ctx, Tool};

/// The tool.
pub struct GetDiffTree;

/// What a caller asks for.
///
/// The doc comments below are read by a model — see the module header.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Args {
    // No doc comment, on purpose: see the module header. The handle writes
    // its own description, which is the same one the tool that minted it
    // shows in its answer.
    pub handle: DiffHandle,

    // Nor on these two: `page` writes their descriptions, and a sentence
    // here would replace the one carrying the numbers that bind.
    #[serde(default)]
    pub cursor: Option<page::Cursor>,

    #[serde(default)]
    pub limit: Option<page::Limit>,
}

/// One file or one directory in the comparison.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, schemars::JsonSchema)]
pub struct Node {
    /// Where it is in the second version, with the archive's top-level
    /// directory removed. For something that was removed, where it was in
    /// the first.
    pub path: String,

    /// Whether this is a file or a directory.
    ///
    /// The part a path does not carry: `src` and `src/index.js` look alike
    /// and only one of them has content to ask for.
    #[serde(rename = "type")]
    pub node_type: NodeType,

    /// What happened to it between the two versions.
    ///
    /// A directory's status summarises what is under it: it is `modified`
    /// when anything beneath it changed, and `unchanged` only when nothing
    /// did.
    pub status: Status,

    /// Lines added. For a directory, the sum of everything under it.
    pub lines_added: u32,

    /// Lines removed. For a directory, the sum of everything under it.
    pub lines_removed: u32,

    /// Where the file was in the first version, when it moved. Absent
    /// unless the status is `renamed`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub old_path: Option<String>,
}

impl Node {
    /// One node of the engine's tree, as this tool lists it.
    fn of(entry: &DiffFileEntry) -> Self {
        Self {
            path: entry.path.clone(),
            node_type: (&entry.file_type).into(),
            status: (&entry.status).into(),
            // The engine leaves these unset on a node it did not compare,
            // which is not the same as comparing one and finding nothing.
            lines_added: entry.added.unwrap_or(0),
            lines_removed: entry.removed.unwrap_or(0),
            old_path: entry.old_path.clone(),
        }
    }
}

/// A file, or a directory holding other nodes.
// Two variants mirroring `engine::FileType`, written here rather than
// re-exported because the engine's type carries no JSON schema and a tool
// declares types, not JSON. The `From` below is a total `match`, so a third
// variant in the engine is a compile error here rather than a value this
// server quietly renames.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum NodeType {
    File,
    Directory,
}

impl From<&FileType> for NodeType {
    fn from(file_type: &FileType) -> Self {
        match file_type {
            FileType::File => Self::File,
            FileType::Directory => Self::Directory,
        }
    }
}

/// What happened to one file or directory between the two versions.
// Five variants mirroring `engine::DiffStatus`, for the reason `NodeType`
// above mirrors `engine::FileType`. It is not the summary tool's `Status`
// either, and that is the part worth saying: this one is *read* as well as
// written — it is what a caller filters by — so it is part of this tool's
// input schema, which is generated from the types this module declares. Both
// mirrors are total `match`es over the engine's enum, so a sixth status
// there is a compile error in each of them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum Status {
    Added,
    Removed,
    Modified,
    Renamed,
    Unchanged,
}

impl From<&DiffStatus> for Status {
    fn from(status: &DiffStatus) -> Self {
        match status {
            DiffStatus::Added => Self::Added,
            DiffStatus::Removed => Self::Removed,
            DiffStatus::Modified => Self::Modified,
            DiffStatus::Renamed => Self::Renamed,
            DiffStatus::Unchanged => Self::Unchanged,
        }
    }
}

/// Every node under `parent`, depth first, in the comparison's own order.
///
/// A directory comes before what is under it, and siblings are in the order
/// the engine put them in — which is by path, so the sequence is the same one
/// twice and a cursor names a position in it rather than in an arrangement
/// that existed for one request.
///
/// `parent` itself is not in the result. See the module header: a directory
/// is not inside its own subtree.
fn flatten(parent: &DiffFileEntry, nodes: &mut Vec<Node>) {
    for child in parent.children.iter().flatten() {
        nodes.push(Node::of(child));
        flatten(child, nodes);
    }
}

impl Tool for GetDiffTree {
    const NAME: &'static str = "get_diff_tree";
    const TITLE: &'static str = "Get diff tree";
    const DESCRIPTION: &'static str = "\
        List the files and directories of a comparison you have already made, \
        a page at a time. Takes the handle `diff_package_versions` gave you \
        and nothing else. Every entry carries its full path, whether it is a \
        file or a directory, what happened to it, where it came from if it \
        moved, and how many lines it gained and lost. A directory's line \
        counts are the sum of everything under it, so counting both files and \
        directories counts every change more than once.";

    /// It downloads and compares; it changes nothing anywhere.
    const READ_ONLY: bool = true;

    /// A handle names two published versions, which are immutable, so the
    /// same arguments always give the same page.
    const IDEMPOTENT: bool = true;

    /// The handle names a package on a registry, which is a world this
    /// server does not control.
    const OPEN_WORLD: bool = true;

    type Args = Args;
    type Output = Page<Node>;

    async fn call(args: Args, ctx: &Ctx) -> Result<Page<Node>, Failure> {
        let inputs = args.handle.inputs();

        // Concurrently, the way the tool that minted this handle fetched
        // them: the two downloads do not depend on each other.
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

        let mut nodes = Vec::new();
        flatten(&tree, &mut nodes);

        page::paginate(nodes, args.limit, args.cursor)
    }
}
