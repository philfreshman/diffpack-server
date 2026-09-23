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
//! # What the walk is rooted at
//!
//! A directory is not inside its own subtree, so neither the comparison's
//! root nor a directory named by `path` is in what comes back — the same
//! rule [`super::list_package_files`]'s `prefix` follows, and for the same
//! reason: an agent that asked what is under `src` is not asking about
//! `src`. Both arguments are one type, [`crate::page::Subtree`], which
//! normalises the directory and writes the rule into the schema, so the two
//! tools cannot read `/` two ways again (#97).
//!
//! # Where a cached result comes in
//!
//! In the first line of the handler, and out of sight in it. The walk from a
//! handle to a comparison is [`super::diff_package_versions::compare`]'s
//! (#83), and looking in the store is the first thing that walk does — so a
//! tree this server has already worked out is paged here without either
//! archive being downloaded a second time. #21 built the store and scoped it
//! to the tool that writes entries; this is the reading half of it, and
//! nothing about it is visible in the code below.
//!
//! What used to be here is the other half of the same walk and has not gone
//! anywhere. A miss recomputes the comparison from the inputs the handle
//! carries, which is what makes an evicted entry a slower answer rather than
//! a refusal (#44, [ADR
//! 0006](../../docs/adr/0006-the-handle-carries-its-inputs.md)) — and a store
//! that could not be read is a miss like any other, so a cache failure never
//! becomes a failure here ([ADR
//! 0003](../../docs/adr/0003-the-cache-seam-is-a-store.md)). Both were the
//! only behaviour this tool had before a store was in front of it, which is
//! why the tests that held them hold them unchanged.
//!
//! # Where the descriptions come from
//!
//! Every doc comment on a field of [`Args`], [`Node`], [`NodeType`] and
//! [`Status`] becomes a `description` in a schema a model reads, so it is
//! written for that reader and names nothing in this repository. Why a field
//! is shaped the way it is belongs here or in an ordinary comment beside the
//! code.
//!
//! Four fields have no doc comment at all, deliberately. `handle` is
//! [`crate::handle`]'s type and `path`, `cursor` and `limit` are
//! [`crate::page`]'s, and those modules write their descriptions — where a
//! handle comes from and that it is not built by hand, what a subtree is and
//! is not, the default and the range of a limit, the rule that a cursor is
//! passed back unchanged. A doc comment here would *replace* those rather
//! than add to them, which is how four tools that take one handle end up
//! describing it four ways.

use serde::{Deserialize, Serialize};

use crate::engine::{DiffFileEntry, DiffStatus, FileType};
use crate::error::Failure;
use crate::handle::DiffHandle;
use crate::page::{self, Page};
use crate::tools::{diff_package_versions, Ctx, Tool};

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

    // No doc comment, on purpose: see the module header. `page` writes the
    // rule for a directory whose subtree is asked for, once, for this and
    // for `list_package_files`' `prefix`.
    #[serde(default)]
    pub path: Option<page::Subtree>,

    /// How many levels to descend, counting from wherever the answer is
    /// rooted — the comparison itself, or the directory `path` named: `1` is
    /// that directory's own contents and nothing inside them. Omit it to
    /// descend the whole way.
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

    /// Only files and directories with one of these statuses. Omit it for
    /// all of them.
    ///
    /// Most of a comparison is `unchanged`: a version bump moves a handful
    /// of files in a package that ships thousands, and paging through the
    /// rest is a call spent on what did not happen. Asking for `added`,
    /// `removed`, `modified` and `renamed` is how you read what changed.
    ///
    /// A directory is kept or dropped on its own status, and a directory's
    /// status is not the union of its children's — one holding a single
    /// changed file is `modified` even though the directory itself did not
    /// move, and one the second version added is `added` even when the only
    /// thing inside it is a file that moved there. Asking for `modified`
    /// alone is not a way to find every directory something happened under.
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
    /// A directory's status is about the directory first and what is under
    /// it second. One the second version does not have is `removed` and one
    /// the first version did not have is `added`, whatever happened to the
    /// files inside — a directory that is new is `added` even when the only
    /// thing in it is a file that moved there. A directory both versions
    /// have is `modified` when anything beneath it changed and `unchanged`
    /// only when nothing did.
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
            // Optional on the engine's type and set on every node of a tree
            // it has built, files and directories alike. Reading an absent
            // one as zero rather than unwrapping it is the fail-safe
            // direction for a field a later engine could leave out: a node
            // that says nothing moved is wrong in a way an agent can see
            // against the totals, where a panic inside a request is not.
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

/// Every node of a comparison, in the order this tool walks them.
///
/// The whole tree: no subtree, no depth, no status filter — which is what
/// "equivalent to `get_diff_tree` over the whole tree" means for the
/// `diffpack://diff/{handle}` resource (#16), the second caller and the
/// reason this is public. The resource answers with the nodes this tool would
/// page through rather than with a second walk of the same tree, so a reader
/// that followed the URI and a reader that followed the cursors see one
/// comparison.
pub fn nodes(tree: &DiffFileEntry) -> Vec<Node> {
    let mut nodes = Vec::new();
    flatten(tree, u32::MAX, &[], &mut nodes);
    nodes
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
/// Public because there are three callers, and all three want the same
/// descent: this tool, which roots a listing at it; the
/// `diffpack://diff/{handle}/file/{path}` resource (#16), which reads a
/// renamed file's `old_path` off it; and
/// [`super::diff_package_versions::Comparison::file_patch`] (#84, #93), which
/// asks the same question of the same field — whether the file a caller named
/// is the one a remembered patch was rendered from — and asks the node's type
/// to refuse a directory without a download. Each of them with a walk of its
/// own would be three ways of finding a node in a tree.
///
/// A descent rather than a scan: at each level only the child whose path is
/// `path` or a directory `path` lies inside is followed, so a subtree of a
/// package with ten thousand files costs one step per directory rather than
/// a walk of everything above it.
///
/// It is a path and not a prefix. Matching against `src/` rather than `src`
/// is what makes `sr` unable to narrow to `src/lib.rs` and `lib` unable to
/// swallow `libs/` — the same distinction [`crate::page::Subtree::contains`]
/// draws for `list_package_files`, where there is no tree to descend.
pub fn node_at<'t>(root: &'t DiffFileEntry, path: &str) -> Option<&'t DiffFileEntry> {
    if root.path == path {
        return Some(root);
    }

    root.children
        .iter()
        .flatten()
        .find(|child| path == child.path || path.starts_with(&format!("{}/", child.path)))
        .and_then(|child| node_at(child, path))
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
        spent on what did not happen. Each file and directory in the answer \
        carries its full path, which of the two it is, what happened to it, \
        where it came from if it moved, and how many lines it gained and \
        lost. A directory's line counts are the sum of everything under it, \
        so counting files and directories together counts every change more \
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
        let tree = diff_package_versions::compare(&args.handle, ctx).await?.tree;

        // An absent path is the root, which is what `/` is: the whole
        // comparison. The Subtree has already normalised what was asked for,
        // so what is left is a directory to descend to, or the root.
        let rooted_at = match args.path.unwrap_or_default().directory() {
            Some(directory) => node_at(&tree, directory),
            None => Some(&tree),
        };

        // An absent depth is as far as there is. A depth of zero is a caller
        // counting from zero, and one level is the nearest thing it can have
        // meant — see the argument's own note.
        let depth = args.depth.map_or(u32::MAX, |asked| asked.max(1));

        let mut nodes = Vec::new();
        if let Some(rooted_at) = rooted_at {
            flatten(rooted_at, depth, &args.status, &mut nodes);
        }

        page::paginate(nodes, args.limit, args.cursor)
    }
}
