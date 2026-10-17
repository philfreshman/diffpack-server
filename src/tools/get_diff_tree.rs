use serde::{Deserialize, Serialize};

use crate::engine::{DiffFileEntry, DiffStatus, FileType};
use crate::error::Failure;
use crate::handle::DiffHandle;
use crate::page::{self, Page};
use crate::tools::{diff_package_versions, Call, Tool};

pub struct GetDiffTree;

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[schemars(description = "What a caller asks for.\n\nThe doc comments below are read by a model — see the module header.")]
#[serde(deny_unknown_fields)]
pub struct Args {
    pub handle: DiffHandle,

    #[serde(default)]
    pub path: Option<page::Subtree>,

    #[schemars(description = "How many levels to descend, counting from wherever the answer is\nrooted — the comparison itself, or the directory `path` named: `1` is\nthat directory's own contents and nothing inside them. Omit it to\ndescend the whole way.")]
    #[schemars(range(min = 1))]
    #[serde(default)]
    pub depth: Option<u32>,

    #[schemars(description = "Only files and directories with one of these statuses. Omit it for\nall of them.\n\nMost of a comparison is `unchanged`: a version bump moves a handful\nof files in a package that ships thousands, and paging through the\nrest is a call spent on what did not happen. Asking for `added`,\n`removed`, `modified` and `renamed` is how you read what changed.\n\nA directory is kept or dropped on its own status, and a directory's\nstatus is not the union of its children's — one holding a single\nchanged file is `modified` even though the directory itself did not\nmove, and one the second version added is `added` even when the only\nthing inside it is a file that moved there. Asking for `modified`\nalone is not a way to find every directory something happened under.")]
    #[serde(default)]
    pub status: Vec<Status>,

    #[serde(default)]
    pub cursor: Option<page::Cursor>,

    #[serde(default)]
    pub limit: Option<page::Limit>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, schemars::JsonSchema)]
#[schemars(description = "One file or one directory in the comparison.")]
pub struct Node {
    #[schemars(description = "Where it is in the second version, with the archive's top-level\ndirectory removed. For something that was removed, where it was in\nthe first.")]
    pub path: String,

    #[schemars(description = "Whether this is a file or a directory.\n\nThe part a path does not carry: `src` and `src/index.js` look alike\nand only one of them has content to ask for.")]
    #[serde(rename = "type")]
    pub node_type: NodeType,

    #[schemars(description = "What happened to it between the two versions.\n\nA directory's status is about the directory first and what is under\nit second. One the second version does not have is `removed` and one\nthe first version did not have is `added`, whatever happened to the\nfiles inside — a directory that is new is `added` even when the only\nthing in it is a file that moved there. A directory both versions\nhave is `modified` when anything beneath it changed and `unchanged`\nonly when nothing did.")]
    pub status: Status,

    #[schemars(description = "Lines added. For a directory, the sum of everything under it.")]
    pub lines_added: u32,

    #[schemars(description = "Lines removed. For a directory, the sum of everything under it.")]
    pub lines_removed: u32,

    #[schemars(description = "Where the file was in the first version, when it moved. Absent\nunless the status is `renamed`.")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub old_path: Option<String>,
}

impl Node {
    fn of(entry: &DiffFileEntry) -> Self {
        Self {
            path: entry.path.clone(),
            node_type: (&entry.file_type).into(),
            status: (&entry.status).into(),
            lines_added: entry.added.unwrap_or(0),
            lines_removed: entry.removed.unwrap_or(0),
            old_path: entry.old_path.clone(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, schemars::JsonSchema)]
#[schemars(description = "A file, or a directory holding other nodes.")]
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[schemars(description = "What happened to one file or directory between the two versions.")]
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

pub fn nodes(tree: &DiffFileEntry) -> Vec<Node> {
    let mut nodes = Vec::new();
    flatten(tree, u32::MAX, &[], &mut nodes);
    nodes
}

fn flatten(parent: &DiffFileEntry, left: u32, wanted: &[Status], nodes: &mut Vec<Node>) {
    if left == 0 {
        return;
    }

    for child in parent.children.iter().flatten() {
        if wanted.is_empty() || wanted.contains(&(&child.status).into()) {
            nodes.push(Node::of(child));
        }
        flatten(child, left - 1, wanted, nodes);
    }
}

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
        for which kinds of change you want. A directory a rename left empty \
        is not in the comparison at all, even though the first version had \
        it, so a `path` naming it is an empty page. Ask for `added`, `removed`, \
        `modified` and `renamed` to read what changed — most of a package is \
        `unchanged` between two versions, and paging through that is a call \
        spent on what did not happen. Each file and directory in the answer \
        carries its full path, which of the two it is, what happened to it, \
        where it came from if it moved, and how many lines it gained and \
        lost. A directory's line counts are the sum of everything under it, \
        so counting files and directories together counts every change more \
        than once.";

    const READ_ONLY: bool = true;

    const IDEMPOTENT: bool = true;

    const OPEN_WORLD: bool = true;

    type Args = Args;
    type Output = Page<Node>;

    async fn call(args: Args, call: &Call) -> Result<Page<Node>, Failure> {
        let tree = diff_package_versions::compare(&args.handle, call).await?.tree;

        let rooted_at = match args.path.unwrap_or_default().directory() {
            Some(directory) => node_at(&tree, directory),
            None => Some(&tree),
        };

        let depth = args.depth.map_or(u32::MAX, |asked| asked.max(1));

        let mut nodes = Vec::new();
        if let Some(rooted_at) = rooted_at {
            flatten(rooted_at, depth, &args.status, &mut nodes);
        }

        page::paginate(nodes, args.limit, args.cursor)
    }
}
