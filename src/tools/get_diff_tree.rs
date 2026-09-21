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

    /// Only what is inside this directory, one level or many: `src`, or
    /// `src/util`. A trailing slash is allowed and makes no difference.
    ///
    /// It names a directory and is not matched by characters, so `sr` does
    /// not narrow to `src/`, and the directory itself is not in its own
    /// subtree. Omit it for the whole comparison. A path with nothing under
    /// it is an empty page rather than an error — which is what a file is,
    /// and what a directory the comparison dropped is.
    #[serde(default)]
    pub path: Option<String>,

    /// How many levels to descend, counting from whatever the listing is
    /// rooted at: `1` is that directory's own contents and nothing inside
    /// them. Omit it to descend the whole way.
    // Declared rather than enforced, the way `similarity_threshold` is on
    // the tool that mints a handle — but the other way round, because this
    // one has a safe reading below its range. A `0` is what counting from
    // zero produces, and the answer it would otherwise get is an empty page
    // that reads as "this directory is empty". So the minimum is in the
    // schema an agent reads, and a value under it is narrowed into range
    // rather than answered literally, which is what `page` does with a
    // limit.
    #[schemars(range(min = 1))]
    #[serde(default)]
    pub depth: Option<u32>,

    /// Only entries with one of these statuses. Omit it for all of them.
    ///
    /// Most of a comparison is `unchanged`: a version bump moves a handful
    /// of files in a package that ships thousands, and paging through the
    /// rest is a call spent on what did not happen. Asking for `added`,
    /// `removed`, `modified` and `renamed` is how you read what changed.
    ///
    /// A directory is kept or dropped on its own status, which summarises
    /// what is under it — so a directory holding one changed file is
    /// `modified` even though the directory itself did not move.
    // A `Vec` rather than an `Option<Vec>`: absent and empty are the same
    // question — "narrow this to nothing in particular" — and the second
    // reading of an empty list, that nothing matches, is an empty page an
    // agent reads as a comparison in which nothing happened.
    #[serde(default)]
    pub status: Vec<Status>,

    // No doc comment on these two either: `page` writes their descriptions,
    // and a sentence here would replace the one carrying the numbers that
    // bind.
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
///
/// `left` is how many levels below `parent` to take, so the recursion stops
/// where the caller asked rather than where the tree ends.
///
/// `wanted` is the statuses to keep, or nothing to keep all of them. It
/// decides what is *listed* and never what is descended into: a directory an
/// agent did not ask for still has the files it holds walked, or asking for
/// added files would answer with the ones at the top of a package and none
/// of the ones inside a directory that was itself unchanged.
fn flatten(parent: &DiffFileEntry, left: u32, wanted: &[Status], nodes: &mut Vec<Node>) {
    if left == 0 {
        return;
    }

    for child in parent.children.iter().flatten() {
        // The status is compared before a node is built, so a filtered walk
        // does not pay for the paths and the clones of what it is about to
        // throw away — which is most of a real comparison.
        if wanted.is_empty() || wanted.contains(&(&child.status).into()) {
            nodes.push(Node::of(child));
        }
        flatten(child, left - 1, wanted, nodes);
    }
}

/// The node `path` names, or nothing if the comparison has no node there.
///
/// A descent rather than a scan: at each level only the child whose path is
/// `path` or a directory `path` lies inside is followed, so a subtree of a
/// package with ten thousand files costs one step per directory rather than
/// a walk of everything above it.
///
/// It is a path and not a prefix. Matching against `src/` rather than `src`
/// is what makes `sr` unable to narrow to `src/lib.rs` and `lib` unable to
/// swallow `libs/` — the same distinction `list_package_files` draws, where
/// it is the whole of what makes the argument name a directory.
fn subtree<'t>(root: &'t DiffFileEntry, path: &str) -> Option<&'t DiffFileEntry> {
    if root.path == path {
        return Some(root);
    }

    root.children
        .iter()
        .flatten()
        .find(|child| path == child.path || path.starts_with(&format!("{}/", child.path)))
        .and_then(|child| subtree(child, path))
}

impl Tool for GetDiffTree {
    const NAME: &'static str = "get_diff_tree";
    const TITLE: &'static str = "Get diff tree";
    const DESCRIPTION: &'static str = "\
        List the files and directories of a comparison you have already made, \
        a page at a time. Takes the handle `diff_package_versions` gave you, \
        and three ways to ask for less than all of it: `path` for one \
        directory's contents, `depth` for how far down to go, and `status` \
        for which kinds of change you want. Ask for `added`, `removed`, \
        `modified` and `renamed` to read what changed — most of a package is \
        `unchanged` between two versions, and paging through that is a call \
        spent on what did not happen. Every entry carries its full path, \
        whether it is a file or a directory, what happened to it, where it \
        came from if it moved, and how many lines it gained and lost. A \
        directory's line counts are the sum of everything under it, so \
        counting files and directories together counts every change more \
        than once.";

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

        // The slash a caller may or may not have written, removed exactly
        // once. An empty one left over is the whole comparison, which is
        // what `/` means and what omitting the argument means.
        let listing = match args.path.as_deref().map(|path| path.trim_end_matches('/')) {
            Some(path) if !path.is_empty() => subtree(&tree, path),
            _ => Some(&tree),
        };

        // An absent depth is as far as there is. A depth of zero is a caller
        // counting from zero, and one level is the nearest thing it can have
        // meant — see the argument's own note.
        let depth = args.depth.map_or(u32::MAX, |asked| asked.max(1));

        let mut nodes = Vec::new();
        if let Some(listing) = listing {
            flatten(listing, depth, &args.status, &mut nodes);
        }

        page::paginate(nodes, args.limit, args.cursor)
    }
}
