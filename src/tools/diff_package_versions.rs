//! `diff_package_versions` — what changed between two published versions.
//!
//! The centre of the server: every other diff tool reads back something this
//! one computed. It answers with totals and a handle, never with the tree —
//! a large package's tree does not fit under the response ceiling, and an
//! answer that did fit would still spend an agent's context on a list it did
//! not ask for. `get_diff_tree` (#14) walks it, a page at a time.
//!
//! # Why a handle rather than a diff_id
//!
//! Specification revision `2026-07-28` removed sessions, so state that
//! crosses calls travels as an argument. A bare `diff_id` cannot be read
//! back — it is a hash — so a reading tool holding one and finding nothing
//! cached would have to refuse. The handle carries the inputs beside the
//! `diff_id`, which turns that refusal into a recompute. See [ADR
//! 0006](../../docs/adr/0006-the-handle-carries-its-inputs.md).
//!
//! The bare `diff_id` is returned beside it anyway, because #27 looks a
//! result up by exactly that string.
//!
//! # Where the descriptions come from
//!
//! Every doc comment on a field of [`Args`] and [`Output`] becomes a
//! `description` in a schema a model reads, so it is written for that reader
//! and names nothing in this repository. Why a field is shaped the way it is
//! belongs here or in an ordinary comment beside the code.

use serde::{Deserialize, Serialize};

use crate::engine::{self, DiffFileEntry, DiffStatus, FileType};
use crate::error::Failure;
use crate::handle::{DiffHandle, Inputs};
use crate::registry::Registry;
use crate::tools::{Ctx, Tool};

/// The tool.
pub struct DiffPackageVersions;

/// The rename threshold `diffpack`'s own worker passes.
///
/// Written as a function because it is a `serde` default and a schema
/// default at once: the number an agent is shown and the number an omitted
/// argument gets are the same one, which matters because it is part of the
/// `diff_id` the agent is handed back.
fn default_similarity_threshold() -> f64 {
    0.75
}

/// What a caller asks for.
///
/// Nothing is normalised: the package name and both versions go to the
/// registry exactly as they arrive, the rule `docs/cache-key.md` fixes for
/// the cache key. The doc comments below are read by a model — see the
/// module header.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Args {
    /// The registry that publishes the package.
    // The enum itself comes from `crate::registry`, so the list an agent is
    // shown is the list this server has rather than a description of one.
    pub registry: Registry,

    /// The package name as the registry spells it, scope included:
    /// `zod`, `@types/node`, `serde`.
    pub package: String,

    /// The version to compare from — the older one, normally. Spelled the
    /// way the registry spells it: `4.0.0`. Not a range, not a tag.
    pub from_version: String,

    /// The version to compare to. Order matters: comparing `1.0.0` to
    /// `2.0.0` is not the same as comparing `2.0.0` to `1.0.0`, and the two
    /// have different identifiers.
    pub to_version: String,

    /// How alike a removed file and an added file must be before the pair is
    /// reported as one renamed file, from `0` to `1`. Lower it to find
    /// renames in files that also changed a lot.
    #[serde(default = "default_similarity_threshold")]
    pub similarity_threshold: f64,

    /// Disregard whitespace when comparing, so that a reformatting is not
    /// reported as a change. Every space, tab and line ending is ignored.
    #[serde(default)]
    pub ignore_whitespace: bool,
}

/// What changed, and how to ask for more of it.
#[derive(Debug, Serialize, schemars::JsonSchema)]
pub struct Output {
    /// Pass this back to any tool that reads part of this diff. Treat it as
    /// opaque: it is issued here and passed on unchanged.
    pub handle: String,

    /// The identifier of this comparison, as 64 lowercase hexadecimal
    /// characters. Every argument you passed names it, not the package and
    /// the two versions alone: the same pair compared at a different
    /// similarity threshold, or with whitespace ignored, is a different
    /// comparison and is given a different identifier.
    pub diff_id: String,

    /// The version compared from.
    pub from_version: String,

    /// The version compared to.
    pub to_version: String,

    /// How much changed, in files and in lines.
    pub totals: Totals,

    /// The files that changed most, most first. Unchanged files are not
    /// listed. This is a sample and not the whole comparison: ask for the
    /// full tree if you need every file.
    pub most_changed: Vec<Changed>,
}

/// One file that changed, as the summary lists it.
#[derive(Debug, Serialize, schemars::JsonSchema)]
pub struct Changed {
    /// Where the file is in the second version, with the archive's
    /// top-level directory removed. For a removed file, where it was in the
    /// first.
    pub path: String,

    /// What happened to it.
    pub status: Status,

    /// Lines added to this file.
    pub lines_added: u32,

    /// Lines removed from this file.
    pub lines_removed: u32,

    /// Where the file was in the first version, when it moved. Absent
    /// unless the status is `renamed`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub old_path: Option<String>,
}

/// What happened to one file between the two versions.
// Five variants mirroring `engine::DiffStatus`, written here rather than
// re-exported because the engine's type carries no JSON schema and a tool
// declares types, not JSON. The `From` below is a total `match`, so a sixth
// variant in the engine is a compile error here rather than a value this
// server quietly renames.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, schemars::JsonSchema)]
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

/// How many of the most-changed files the summary lists.
///
/// A sample, not a page: the whole comparison is what `get_diff_tree` is
/// for, and this is the handful an agent reads to decide whether to ask for
/// it. Small enough that no package pair can push this answer near the
/// response ceiling, which is why this tool does not paginate.
const MOST_CHANGED: usize = 20;

/// How much changed between the two versions.
///
/// The five status counts are files, not directories: a directory's status is
/// a summary of what is under it and counting both would report every change
/// more than once.
#[derive(Debug, Default, Serialize, schemars::JsonSchema)]
pub struct Totals {
    /// Files present in the second version and not the first.
    pub added: u32,

    /// Files present in the first version and not the second.
    pub removed: u32,

    /// Files present in both versions whose content differs.
    pub modified: u32,

    /// Files that moved to a different path. Counted here and not under
    /// added or removed.
    pub renamed: u32,

    /// Files present in both versions with identical content.
    pub unchanged: u32,

    /// Lines added across every file.
    pub lines_added: u32,

    /// Lines removed across every file.
    pub lines_removed: u32,
}

impl Totals {
    /// Count one file into the totals.
    ///
    /// A `match` over every status rather than a default arm, so that a sixth
    /// status in the engine is a compile error here instead of a file this
    /// server quietly counts as nothing.
    fn count(&mut self, file: &DiffFileEntry) {
        match file.status {
            DiffStatus::Added => self.added += 1,
            DiffStatus::Removed => self.removed += 1,
            DiffStatus::Modified => self.modified += 1,
            DiffStatus::Renamed => self.renamed += 1,
            DiffStatus::Unchanged => self.unchanged += 1,
        }

        // The engine leaves these unset on a node it did not compare, which
        // is not the same as comparing one and finding nothing.
        self.lines_added += file.added.unwrap_or(0);
        self.lines_removed += file.removed.unwrap_or(0);
    }
}

/// Add every file under `node` to `totals`, collecting the ones that changed.
///
/// Directories are walked and not counted. The engine gives a directory the
/// sum of what is beneath it, so counting one would report the same change
/// twice — once on the file and once on every directory above it.
fn walk(node: &DiffFileEntry, totals: &mut Totals, changed: &mut Vec<Changed>) {
    match node.file_type {
        FileType::File => {
            totals.count(node);

            // An unchanged file is in the totals and not in the listing: an
            // agent asking what changed should not have to filter the answer.
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

/// How much one file moved: the number the listing is ranked by.
fn churn(file: &Changed) -> u32 {
    file.lines_added + file.lines_removed
}

impl Tool for DiffPackageVersions {
    const NAME: &'static str = "diff_package_versions";
    const TITLE: &'static str = "Diff package versions";
    const DESCRIPTION: &'static str = "\
        Compare two published versions of a package and summarise what \
        changed between them. Takes a registry, a package name and two exact \
        versions, all spelled the way the registry spells them. Order \
        matters: from `1.0.0` to `2.0.0` is not the same comparison as from \
        `2.0.0` to `1.0.0`. Answers with a summary and a handle you pass to \
        the tools that read the comparison in detail — not with the whole \
        list of changed files, which can be far too large to return at once.";

    /// It downloads and compares; it changes nothing anywhere.
    const READ_ONLY: bool = true;

    /// Two published versions are immutable, so the same arguments always
    /// give the same comparison.
    const IDEMPOTENT: bool = true;

    /// The arguments name a package on a registry, which is a world this
    /// server does not control.
    const OPEN_WORLD: bool = true;

    type Args = Args;
    type Output = Output;

    async fn call(args: Args, ctx: &Ctx) -> Result<Output, Failure> {
        let handle = DiffHandle::mint(Inputs {
            registry: args.registry,
            package: args.package,
            from_version: args.from_version,
            to_version: args.to_version,
            similarity_threshold: args.similarity_threshold,
            ignore_whitespace: args.ignore_whitespace,
        });
        let inputs = handle.inputs();

        // Concurrently, the way the engine's wasm entry point fetches them:
        // the two downloads do not depend on each other, and a version pair
        // is the one place this server waits on the network twice.
        let (from_files, to_files) = futures::try_join!(
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

        let mut totals = Totals::default();
        let mut changed = Vec::new();
        walk(&tree, &mut totals, &mut changed);

        // Most-moved first, and then by path. The second half is what makes
        // this an order rather than a tendency: a tree walk has its own
        // order, and two files with equal churn would otherwise swap places
        // between two calls that are supposed to be the same diff.
        changed.sort_by(|a, b| churn(b).cmp(&churn(a)).then_with(|| a.path.cmp(&b.path)));
        changed.truncate(MOST_CHANGED);

        Ok(Output {
            handle: handle.encode(),
            diff_id: handle.diff_id(),
            from_version: inputs.from_version.clone(),
            to_version: inputs.to_version.clone(),
            totals,
            most_changed: changed,
        })
    }
}
