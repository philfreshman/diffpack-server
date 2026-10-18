use std::collections::BTreeMap;

use futures::try_join;
use rmcp::model::Resource;
use serde::{Deserialize, Serialize};

use crate::archive::{At, FileMap};
use crate::engine::{self, DiffFileEntry, DiffStatus, FileType, Patch};
use crate::error::Failure;
use crate::handle::{DiffHandle, Inputs};
use crate::registry::{PackageName, Registry, VersionName};
use crate::resources;
use crate::store::Entry;
use crate::tools::get_file_diff::{self, OneFile};
use crate::tools::get_diff_tree::{self, NodeType};
use crate::tools::{Call, Tool};

pub struct DiffPackageVersions;

fn default_similarity_threshold() -> f64 {
    0.75
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[schemars(
    description = "What a caller asks for.\n\
    \n\
    Nothing is normalised: the package name and both versions go to the\n\
    registry exactly as they arrive, the rule `docs/cache-key.md` fixes for\n\
    the cache key. The doc comments below are read by a model — see the\n\
    module header."
)]
#[serde(deny_unknown_fields)]
pub struct Args {
    #[schemars(description = "The registry that publishes the package.")]
    pub registry: Registry,

    pub package: PackageName,

    pub from_version: VersionName,

    pub to_version: VersionName,

    #[schemars(
        description = "How alike a removed file and an added file must be before the pair is\n\
        reported as one renamed file, from `0` to `1`. Lower it to find\n\
        renames in files that also changed a lot."
    )]
    #[schemars(range(min = 0.0, max = 1.0))]
    #[serde(default = "default_similarity_threshold")]
    pub similarity_threshold: f64,

    #[schemars(
        description = "Disregard whitespace when comparing, so that a reformatting is not\n\
        reported as a change. Every space, tab and line ending is ignored."
    )]
    #[serde(default)]
    pub ignore_whitespace: bool,
}

#[derive(Debug, Serialize, schemars::JsonSchema)]
#[schemars(description = "What changed, and how to ask for more of it.")]
pub struct Output {
    pub handle: DiffHandle,

    #[schemars(
        description = "The identifier of this comparison, as 64 lowercase hexadecimal\n\
        characters. Every argument you passed names it, not the package and\n\
        the two versions alone: the same pair compared at a different\n\
        similarity threshold, or with whitespace ignored, is a different\n\
        comparison and is given a different identifier."
    )]
    pub diff_id: String,

    #[schemars(description = "The version compared from.")]
    pub from_version: String,

    #[schemars(description = "The version compared to.")]
    pub to_version: String,

    #[schemars(description = "How much changed, in files and in lines.")]
    pub totals: Totals,

    #[schemars(
        description = "The files that changed most, most first. Unchanged files are not\n\
        listed. This is a sample and not the whole comparison: ask for the\n\
        full tree if you need every file."
    )]
    pub most_changed: Vec<Changed>,

    #[schemars(
        description = "Whether this comparison was remembered from an earlier call rather\n\
        than worked out now. A remembered one costs no downloads, so asking\n\
        again for something you have already asked for is cheap."
    )]
    pub cached: bool,
}

#[derive(Debug, Serialize, schemars::JsonSchema)]
#[schemars(description = "One file that changed, as the summary lists it.")]
pub struct Changed {
    #[schemars(
        description = "Where the file is in the second version, with the archive's\n\
        top-level directory removed. For a removed file, where it was in the\n\
        first."
    )]
    pub path: String,

    #[schemars(description = "What happened to it.")]
    pub status: Status,

    #[schemars(description = "Lines added to this file.")]
    pub lines_added: u32,

    #[schemars(description = "Lines removed from this file.")]
    pub lines_removed: u32,

    #[schemars(
        description = "Where the file was in the first version, when it moved. Absent\n\
        unless the status is `renamed`."
    )]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub old_path: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, schemars::JsonSchema)]
#[schemars(description = "What happened to one file between the two versions.")]
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

const MOST_CHANGED: usize = 20;

#[derive(Debug, Default, Serialize, schemars::JsonSchema)]
#[schemars(
    description = "How much changed between the two versions.\n\
    \n\
    The five status counts are files, not directories: a directory's status is\n\
    a summary of what is under it and counting both would report every change\n\
    more than once."
)]
pub struct Totals {
    #[schemars(description = "Files present in the second version and not the first.")]
    pub added: u32,

    #[schemars(description = "Files present in the first version and not the second.")]
    pub removed: u32,

    #[schemars(description = "Files present in both versions whose content differs.")]
    pub modified: u32,

    #[schemars(
        description = "Files that moved to a different path. Counted here and not under\n\
        added or removed."
    )]
    pub renamed: u32,

    #[schemars(description = "Files present in both versions with identical content.")]
    pub unchanged: u32,

    #[schemars(description = "Lines added across every file.")]
    pub lines_added: u32,

    #[schemars(description = "Lines removed across every file.")]
    pub lines_removed: u32,
}

impl Totals {
    fn count(&mut self, file: &DiffFileEntry) {
        match file.status {
            DiffStatus::Added => self.added += 1,
            DiffStatus::Removed => self.removed += 1,
            DiffStatus::Modified => self.modified += 1,
            DiffStatus::Renamed => self.renamed += 1,
            DiffStatus::Unchanged => self.unchanged += 1,
        }

        self.lines_added += file.added.unwrap_or(0);
        self.lines_removed += file.removed.unwrap_or(0);
    }
}

pub fn totals(tree: &DiffFileEntry) -> Totals {
    let mut totals = Totals::default();
    walk(tree, &mut totals, &mut Vec::new());
    totals
}

fn walk(node: &DiffFileEntry, totals: &mut Totals, changed: &mut Vec<Changed>) {
    match node.file_type {
        FileType::File => {
            totals.count(node);

            if node.status != DiffStatus::Unchanged {
                changed.push(Changed {
                    path: node.path.clone(),
                    status: (&node.status).into(),
                    lines_added: node.added.unwrap_or(0),
                    lines_removed: node.removed.unwrap_or(0),
                    old_path: node.old_path.clone(),
                });
            }
        }
        FileType::Directory => {
            for child in node.children.iter().flatten() {
                walk(child, totals, changed);
            }
        }
    }
}

fn rendered(node: &DiffFileEntry, files: &Versions, ignore_whitespace: bool) -> BTreeMap<String, Patch> {
    let mut patches = BTreeMap::new();
    collect(node, files, ignore_whitespace, &mut patches);
    patches
}

fn collect(
    node: &DiffFileEntry,
    files: &Versions,
    ignore_whitespace: bool,
    patches: &mut BTreeMap<String, Patch>,
) {
    match node.file_type {
        FileType::File => {
            if node.status == DiffStatus::Unchanged {
                return;
            }

            let was = node.old_path.as_deref().unwrap_or(&node.path);
            patches.insert(
                node.path.clone(),
                patch_of(files, &node.path, was, ignore_whitespace),
            );
        }
        FileType::Directory => {
            for child in node.children.iter().flatten() {
                collect(child, files, ignore_whitespace, patches);
            }
        }
    }
}

fn patch_of(files: &Versions, path: &str, from_path: &str, ignore_whitespace: bool) -> Patch {
    engine::build_patch(
        path,
        files.from_files.at(from_path).text(),
        files.to_files.at(path).text(),
        ignore_whitespace,
    )
}

fn churn(file: &Changed) -> u32 {
    file.lines_added + file.lines_removed
}

struct Versions {
    from_files: FileMap,
    to_files: FileMap,
}

pub struct Comparison {
    pub tree: DiffFileEntry,

    pub cached: bool,

    handle: DiffHandle,

    patches: BTreeMap<String, Patch>,

    files: Option<Versions>,
}

impl Comparison {
    pub async fn file_patch(
        &self,
        call: &Call,
        asked: OneFile<'_>,
    ) -> Result<get_file_diff::Patch, Failure> {
        let from_path = asked.old_path.unwrap_or(asked.path);
        refuse_directory(self.handle.inputs(), asked.path, from_path, |end, path| {
            kind_in_tree(&self.tree, end, path)
        })?;

        if let Some(patch) = self.stored(asked.path, asked.old_path) {
            return Ok(get_file_diff::presented(patch, &asked));
        }

        let rendered = self.rendered(call, asked.path, asked.old_path).await?;

        Ok(get_file_diff::presented(&rendered, &asked))
    }

    fn stored(&self, path: &str, old_path: Option<&str>) -> Option<&Patch> {
        let node = get_diff_tree::node_at(&self.tree, path, NodeType::File)?;

        (node.old_path.as_deref() == old_path)
            .then(|| self.patches.get(path))
            .flatten()
    }

    async fn rendered(
        &self,
        call: &Call,
        path: &str,
        old_path: Option<&str>,
    ) -> Result<Patch, Failure> {
        let inputs = self.handle.inputs();
        let from_path = old_path.unwrap_or(path);

        let fetched;
        let files = match &self.files {
            Some(files) => files,
            None => {
                fetched = versions(&self.handle, call).await?;
                &fetched
            }
        };

        refuse_directory(inputs, path, from_path, |end, path| files.kind_at(end, path))?;

        Ok(patch_of(files, path, from_path, inputs.ignore_whitespace))
    }
}

#[derive(Debug, Clone, Copy)]
enum End {
    From,
    To,
}

impl Versions {
    fn kind_at(&self, end: End, path: &str) -> Option<NodeType> {
        let files = match end {
            End::From => &self.from_files,
            End::To => &self.to_files,
        };

        match files.at(path) {
            At::File(_) => Some(NodeType::File),
            At::Directory => Some(NodeType::Directory),
            At::Nothing => None,
        }
    }
}

fn kind_in_tree(tree: &DiffFileEntry, end: End, path: &str) -> Option<NodeType> {
    let absent: &[DiffStatus] = match end {
        End::From => &[DiffStatus::Added, DiffStatus::Renamed],
        End::To => &[DiffStatus::Removed],
    };

    [NodeType::File, NodeType::Directory].into_iter().find(|kind| {
        get_diff_tree::node_at(tree, path, *kind).is_some_and(|node| !absent.contains(&node.status))
    })
}

fn refuse_directory(
    inputs: &Inputs,
    path: &str,
    from_path: &str,
    kind_at: impl Fn(End, &str) -> Option<NodeType>,
) -> Result<(), Failure> {
    let ends = [
        (End::To, path, &inputs.to_version),
        (End::From, from_path, &inputs.from_version),
    ]
    .map(|(end, path, version)| (kind_at(end, path), path, version));

    if ends.iter().any(|(kind, ..)| *kind == Some(NodeType::File)) {
        return Ok(());
    }

    match ends.into_iter().find(|(kind, ..)| *kind == Some(NodeType::Directory)) {
        Some((_, path, version)) => Err(Failure::PathIsDirectory {
            package: inputs.package.clone(),
            version: version.clone(),
            path: path.to_owned(),
        }),
        None => Ok(()),
    }
}

async fn versions(handle: &DiffHandle, call: &Call) -> Result<Versions, Failure> {
    let inputs = handle.inputs();

    let (from_files, to_files) = try_join!(
        call.archive()
            .fetch(inputs.registry, &inputs.package, &inputs.from_version),
        call.archive()
            .fetch(inputs.registry, &inputs.package, &inputs.to_version),
    )?;

    Ok(Versions {
        from_files,
        to_files,
    })
}

pub async fn compare(handle: &DiffHandle, call: &Call) -> Result<Comparison, Failure> {
    let key = handle.key();

    if let Some(entry) = call.store().get(&key).await {
        return Ok(Comparison {
            tree: entry.tree,
            cached: true,
            handle: handle.clone(),
            patches: entry.patches,
            files: None,
        });
    }

    let inputs = handle.inputs();
    let files = versions(handle, call).await?;

    let tree = engine::build_diff_tree(
        &files.from_files,
        &files.to_files,
        inputs.similarity_threshold,
        inputs.ignore_whitespace,
    );

    let patches = rendered(&tree, &files, inputs.ignore_whitespace);

    call.store().put(Entry {
        key,
        tree: tree.clone(),
        patches,
    });

    Ok(Comparison {
        tree,
        cached: false,
        handle: handle.clone(),
        patches: BTreeMap::new(),
        files: Some(files),
    })
}

impl Tool for DiffPackageVersions {
    const NAME: &'static str = "diff_package_versions";
    const TITLE: &'static str = "Diff package versions";
    const DESCRIPTION: &'static str = "\
        Compare two published versions of a package and summarise what \
        changed between them. Takes a registry, a package name and two exact \
        versions, all spelled the way the registry spells them. Order \
        matters: the comparison runs from `from_version` to `to_version`, \
        normally the older to the newer, and swapping the two is a different \
        comparison with a different handle — what one reports as added, the \
        other reports as removed. Answers with a summary and a handle you \
        pass to the tools that read the comparison in detail — not with the \
        whole list of changed files, which can be far too large to return at \
        once.";

    const READ_ONLY: bool = true;

    const IDEMPOTENT: bool = true;

    const OPEN_WORLD: bool = true;

    type Args = Args;
    type Output = Output;

    fn links(output: &Output) -> Vec<Resource> {
        vec![resources::diff::link(&output.handle)]
    }

    async fn call(args: Args, call: &Call) -> Result<Output, Failure> {
        let handle = DiffHandle::mint(Inputs {
            registry: args.registry,
            package: args.package.into(),
            from_version: args.from_version.into(),
            to_version: args.to_version.into(),
            similarity_threshold: args.similarity_threshold,
            ignore_whitespace: args.ignore_whitespace,
        });
        let comparison = compare(&handle, call).await?;

        let mut totals = Totals::default();
        let mut changed = Vec::new();
        walk(&comparison.tree, &mut totals, &mut changed);

        changed.sort_by(|a, b| churn(b).cmp(&churn(a)).then_with(|| a.path.cmp(&b.path)));
        changed.truncate(MOST_CHANGED);

        let diff_id = handle.diff_id();
        let from_version = handle.inputs().from_version.clone();
        let to_version = handle.inputs().to_version.clone();

        Ok(Output {
            handle,
            diff_id,
            from_version,
            to_version,
            totals,
            most_changed: changed,
            cached: comparison.cached,
        })
    }
}
