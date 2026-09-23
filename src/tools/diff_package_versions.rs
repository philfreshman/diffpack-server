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
//! # What else is here, and why it is here
//!
//! [`compare`] — the walk from a handle to a compared tree, with the store on
//! this side of it. It is the whole of what a diff costs, and the three paths
//! that *read* a comparison back go through it: [`super::get_diff_tree`],
//! [`super::get_file_diff`] and the two resources under
//! [`crate::resources::diff`]. It was written four times before #83, which is
//! four places for the pair of downloads, the rename threshold and the
//! whitespace rule to stop agreeing.
//!
//! Here rather than in a module of its own because this is the tool that
//! *computes* a comparison — the other three read back what it worked out —
//! and a shared module named after neither is the alternative [ADR
//! 0014](../../docs/adr/0014-a-resource-is-a-projection-of-the-tools.md)
//! rejected for the walks it lists. Leaving it in `src/resources/` and having
//! three tools call into a resource is the alternative [ADR
//! 0016](../../docs/adr/0016-the-walk-to-a-comparison-is-this-tools.md)
//! rejects, and the one 0014's own sentence forbids. [`compare`] is therefore
//! the fifth export of a tool module with a caller outside it, which 0014
//! names as the direction to watch: the thing it warns about is a resource
//! doing its own work through a tool's front door, and this is work leaving a
//! resource rather than arriving at one.
//!
//! [`Comparison`] is what it answers with, and it carries one of two halves
//! beside the tree: both versions' files where this call worked the
//! comparison out, and every changed file's patch where it came out of the
//! store.
//!
//! [`Comparison::file_patch`] — one file's patch out of it, for
//! `get_file_diff` and the `diffpack://diff/{handle}/file/{path}` resource.
//! The stored patch, a directory refused out of the tree, both versions'
//! files only for a file with neither, and the file rendered through the same
//! lookup the pre-render in [`compare`] uses. Before #93 the two callers each
//! wrote those steps out, in the same order, and the resource did it through
//! three of `get_file_diff`'s exports. It is here because the comparison
//! keeps its handle to itself: the files it fetches are only right for that
//! handle, and a method is the one place the two meet without a caller
//! passing the handle back in.
//!
//! Two things here go to another tool module, and both are that module's own
//! rule rather than work this one could do. [`super::get_diff_tree::node_at`]
//! says where a file sits in the tree, which is how a stored patch is matched
//! to the pair of paths it was rendered from and how a directory is known
//! without a download; that export already had two callers outside its
//! module. [`super::get_file_diff::presented`] is the trim and the cut, which
//! are `get_file_diff`'s because `context_lines` is its argument.
//!
//! What that did to the count [ADR
//! 0014](../../docs/adr/0014-a-resource-is-a-projection-of-the-tools.md) and
//! [ADR 0016](../../docs/adr/0016-the-walk-to-a-comparison-is-this-tools.md)
//! keep — the things a tool module makes public for a caller in another
//! module, beside its own `Args` and answer — is take it from eleven to
//! eight. `Comparison::patch`, `Comparison::files` and `Versions` went from
//! this module and `render` from `get_file_diff`; `file_patch` came. That is
//! 0014's closing paragraph applied rather than argued with: the resource
//! stopped doing its own work through a tool's front door, because the work
//! moved to the module that owns the thing it is about.
//!
//! # Where the descriptions come from
//!
//! Every doc comment on a field of [`Args`] and [`Output`] becomes a
//! `description` in a schema a model reads, so it is written for that reader
//! and names nothing in this repository. Why a field is shaped the way it is
//! belongs here or in an ordinary comment beside the code.
//!
//! `handle` has no doc comment at all, deliberately, and for the reason
//! `max_bytes` has none in [`super::get_file_content`]: it is
//! [`crate::handle`]'s type and that module writes its description — where a
//! handle comes from, that it carries its inputs as well as its `diff_id`,
//! and that a handle whose halves disagree is refused. A sentence here would
//! *replace* that rather than add to it, which is how the tool that mints a
//! handle ends up describing it differently from the three that take one.

use std::collections::BTreeMap;

use futures::try_join;
use rmcp::model::Resource;
use serde::{Deserialize, Serialize};

use crate::archive::FileMap;
use crate::engine::{self, DiffFileEntry, DiffStatus, FileType, Patch};
use crate::error::Failure;
use crate::handle::{DiffHandle, Inputs};
use crate::registry::Registry;
use crate::resources;
use crate::store::Entry;
use crate::tools::get_file_diff::{self, OneFile};
use crate::tools::{get_diff_tree, Ctx, Tool};

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
    // The range is declared rather than enforced, which is the opposite of
    // what `page` does with a limit. The comparison narrows a value outside
    // it, but what names the diff is the number as it arrived: the identifier
    // is a contract with an implementation in another language, so narrowing
    // here would mean two sides computing two names for one comparison. An
    // agent that reads the bound never sends one — which is why the bound is
    // in the schema and not only in the sentence above it.
    #[schemars(range(min = 0.0, max = 1.0))]
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
    // No doc comment, on purpose: see the module header. The handle writes
    // its own description, and this tool's answer is the same type the tools
    // that read a diff back take as an argument — so neither of them says how
    // a handle is spelled, and neither can say it differently.
    pub handle: DiffHandle,

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

    /// Whether this comparison was remembered from an earlier call rather
    /// than worked out now. A remembered one costs no downloads, so asking
    /// again for something you have already asked for is cheap.
    pub cached: bool,
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

/// How much changed across a whole comparison.
///
/// The one place a tree is counted, and public because there are two callers:
/// this tool's summary, and the `diffpack://diff/{handle}` resource (#16),
/// which answers with the same totals by the same walk rather than by a
/// second one. Two walks that disagreed would give an agent two answers to
/// one question with nothing to say which was wrong — the drift ADR 0013
/// records for the patch renderer, in a second place.
///
/// The sample is built and dropped, which is the cost of having one walk
/// rather than two. It is bounded by the number of files that *changed*,
/// which is a handful of a package that ships thousands.
pub fn totals(tree: &DiffFileEntry) -> Totals {
    let mut totals = Totals::default();
    walk(tree, &mut totals, &mut Vec::new());
    totals
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

/// The patch for every file under `node` that changed.
///
/// Unchanged files are left out. An unchanged file's patch is the file, so
/// storing one would be storing the package a second time — and the reading
/// tool that wants it has both versions' contents to hand anyway.
///
/// A renamed file is diffed from where it was: its content in the first
/// version is at `old_path`, and diffing it against itself at its new path
/// would report every line of a moved file as added.
fn rendered(node: &DiffFileEntry, files: &Versions, ignore_whitespace: bool) -> BTreeMap<String, Patch> {
    let mut patches = BTreeMap::new();
    collect(node, files, ignore_whitespace, &mut patches);
    patches
}

/// Add every changed file under `node` to `patches`.
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

/// The patch for the file at `path`, diffed from `from_path` in the first
/// version.
///
/// The one rendering there is, for both of the moments a patch is made: every
/// changed file's, while [`compare`] has both archives in hand, and one file's
/// on demand, when [`Comparison::file_patch`] finds nothing stored for it. A
/// remembered patch and a fresh one only agree while those two read a file
/// the same way, so they read it here rather than each in its own copy.
fn patch_of(files: &Versions, path: &str, from_path: &str, ignore_whitespace: bool) -> Patch {
    engine::patch(
        path,
        content(&files.from_files, from_path),
        content(&files.to_files, path),
        ignore_whitespace,
    )
}

/// What `files` has at `path`, where that is a file at all.
///
/// A directory is nothing, which is the engine's reading: its content is the
/// empty string the extractor gave it. That is why a directory a caller named
/// is refused before anything is rendered — see [`refuse_directory`] — or it
/// would be told the path is in neither version.
fn content<'a>(files: &'a FileMap, path: &str) -> Option<&'a str> {
    files.get(path).and_then(|entry| match entry.file_type {
        FileType::File => Some(entry.content.as_str()),
        FileType::Directory => None,
    })
}

/// How much one file moved: the number the listing is ranked by.
fn churn(file: &Changed) -> u32 {
    file.lines_added + file.lines_removed
}

/// Both versions of a package, as the files each of them ships.
///
/// The pair rather than one and then the other: a comparison is of two
/// versions, and a caller holding one file map has nothing it can say.
struct Versions {
    from_files: FileMap,
    to_files: FileMap,
}

/// One comparison, as whoever asked for it holds it.
///
/// The tree is the whole of the comparison and is always here. What is beside
/// it is one of two halves and never both, and that is the difference this
/// type exists to carry. A comparison worked out now has both versions' files
/// and can render anything out of them. A comparison that came out of the
/// store has the patches that were rendered when the entry was written and
/// none of the archives they came from, because that is what an entry is.
///
/// Neither half is public. Which one a comparison arrived with is not a
/// question a caller should be answering — [`Comparison::file_patch`] is the
/// one question there is about a file, and a caller that matched on the
/// halves would be a second place deciding what to do about a comparison that
/// came without its archives.
pub struct Comparison {
    /// The whole comparison, as the engine arranged it.
    pub tree: DiffFileEntry,

    /// Whether this was remembered rather than worked out now.
    pub cached: bool,

    /// Which comparison this is.
    ///
    /// Carried rather than asked for again. [`Comparison::file_patch`]
    /// fetches two archives when this one was remembered without them, and a
    /// caller that handed it a different handle from the one [`compare`] was
    /// given would be served another comparison's versions with nothing in
    /// the answer saying they do not belong to this tree. The type is where
    /// that is settled rather than in a sentence asking callers to keep
    /// passing the same one.
    ///
    /// It is the handle and not the key, because what a fetch needs is the
    /// inputs a handle carries ([ADR
    /// 0006](../../docs/adr/0006-the-handle-carries-its-inputs.md)) and a key
    /// is the hash of them.
    handle: DiffHandle,

    /// Every changed file's patch, where this comparison was remembered with
    /// them.
    ///
    /// Empty when it was worked out now, and not because there are none: the
    /// call that worked it out holds both versions' files and renders what it
    /// wants from those. This is the half an entry has instead of them.
    patches: BTreeMap<String, Patch>,

    /// Both versions' files, where this call is the one that downloaded them.
    files: Option<Versions>,
}

impl Comparison {
    /// The patch for one file of this comparison, presented the way the
    /// caller asked for it.
    ///
    /// The one answer to "what did this file change", for the two places
    /// that ask: `get_file_diff` and the `diffpack://diff/{handle}/file/{path}`
    /// resource. Behind it, in this order:
    ///
    /// 1. The patch this comparison is already holding for the file, where it
    ///    was remembered with one. A warm call that gets one fetches nothing.
    /// 2. A directory the tree names is refused. A directory never has a
    ///    patch, so the first step answers nothing for one, and the tree
    ///    already says what it is — two downloads would only say it again.
    /// 3. Both versions' files, downloaded now if this comparison was
    ///    remembered without them, and the file rendered out of them.
    /// 4. The trim and the cut, which are `get_file_diff`'s
    ///    ([`get_file_diff::presented`]), for a patch from either step.
    ///
    /// Asking per file and rendering when there is nothing stored is the
    /// whole of the rule, and it is right whichever way a patch went missing
    /// — over the per-patch cap, dropped with the rest because the entry was
    /// too big, written before entries carried patches, or a file that did
    /// not change and so never had one. It never asks whether the entry kept
    /// its patches, and [`crate::store::DiffStore::get`] serving an entry
    /// that lost them rests on that.
    ///
    /// A method rather than a function taking the handle, because a
    /// comparison's files are only right for its own handle. Step 3 is the
    /// one place the two meet, and it is inside the type that holds both.
    pub async fn file_patch(
        &self,
        ctx: &Ctx,
        asked: OneFile<'_>,
    ) -> Result<get_file_diff::Patch, Failure> {
        if let Some(patch) = self.stored(asked.path, asked.old_path) {
            return Ok(get_file_diff::presented(patch, &asked));
        }

        let rendered = self.rendered(ctx, asked.path, asked.old_path).await?;

        Ok(get_file_diff::presented(&rendered, &asked))
    }

    /// The patch this comparison is already holding for the file at `path`,
    /// if it is the patch a caller asking about that file would be rendered.
    ///
    /// `old_path` is where the file was in the first version as the *caller*
    /// named it, and it has to be where this comparison's own tree says it
    /// was. A patch is rendered from one path in each version, so a pair that
    /// does not match the tree's is a different answer and not a worse way of
    /// spelling this one: a renamed file asked about without its `old_path`
    /// is every line of it added, which is what `get_file_diff` has always
    /// said and has to keep saying whether or not anything asked before.
    fn stored(&self, path: &str, old_path: Option<&str>) -> Option<&Patch> {
        let node = get_diff_tree::node_at(&self.tree, path)?;

        (node.old_path.as_deref() == old_path)
            .then(|| self.patches.get(path))
            .flatten()
    }

    /// The file at `path` rendered from both versions, diffed from
    /// `old_path` in the first where the caller named one.
    ///
    /// A directory is refused twice over, and on purpose. The tree is asked
    /// first, because it is here on every comparison and asking it costs no
    /// download. The file maps are asked again once they are in hand, for the
    /// directory the tree does not have: the engine drops one with nothing
    /// under it, which is what a rename leaves of the directory it moved out
    /// of.
    async fn rendered(
        &self,
        ctx: &Ctx,
        path: &str,
        old_path: Option<&str>,
    ) -> Result<Patch, Failure> {
        let inputs = self.handle.inputs();
        let from_path = old_path.unwrap_or(path);

        refuse_directory(inputs, path, from_path, |end, path| {
            get_diff_tree::node_at(&self.tree, path).is_some_and(|node| {
                // A directory's status says which versions have it: `added`
                // is the second alone and `removed` the first alone.
                let absent = match end {
                    End::From => DiffStatus::Added,
                    End::To => DiffStatus::Removed,
                };
                matches!(node.file_type, FileType::Directory) && node.status != absent
            })
        })?;

        let fetched;
        let files = match &self.files {
            Some(files) => files,
            None => {
                fetched = versions(&self.handle, ctx).await?;
                &fetched
            }
        };

        refuse_directory(inputs, path, from_path, |end, path| {
            files
                .of(end)
                .get(path)
                .is_some_and(|entry| matches!(entry.file_type, FileType::Directory))
        })?;

        Ok(patch_of(files, path, from_path, inputs.ignore_whitespace))
    }
}

/// One end of a comparison: the version it is from, or the one it is to.
#[derive(Debug, Clone, Copy)]
enum End {
    From,
    To,
}

impl Versions {
    /// The files `end` ships.
    fn of(&self, end: End) -> &FileMap {
        match end {
            End::From => &self.from_files,
            End::To => &self.to_files,
        }
    }
}

/// The refusal for a caller that named a directory, if it did.
///
/// A directory has no content, so the engine reads one as absent on both
/// sides and renders the sentence that says it is in neither version. That
/// sentence is false about a path the package ships, and an agent has
/// nothing in the answer to doubt it with — the failure `get_file_content`
/// refuses a directory to avoid.
///
/// `is_directory` is where the answer comes from, because there are two
/// places to ask and the refusal must not depend on which one answered: the
/// tree, before anything is downloaded, and the file maps, once they are in
/// hand. The second version first, because that is the one a caller's path
/// usually names.
fn refuse_directory(
    inputs: &Inputs,
    path: &str,
    from_path: &str,
    is_directory: impl Fn(End, &str) -> bool,
) -> Result<(), Failure> {
    let directory = [
        (End::To, path, &inputs.to_version),
        (End::From, from_path, &inputs.from_version),
    ]
    .into_iter()
    .find(|(end, path, _)| is_directory(*end, path));

    match directory {
        Some((_, path, version)) => Err(Failure::PathIsDirectory {
            package: inputs.package.clone(),
            version: version.clone(),
            path: path.to_owned(),
        }),
        None => Ok(()),
    }
}

/// Both versions of what `handle` names, downloaded together.
///
/// The one `try_join!` in this crate, and the reason `futures` is on the list
/// a tool may import. The two downloads do not depend on each other and a
/// version pair is the only place this server waits on the network twice, so
/// waiting on them one after the other would double the wait for nothing.
async fn versions(handle: &DiffHandle, ctx: &Ctx) -> Result<Versions, Failure> {
    let inputs = handle.inputs();

    let (from_files, to_files) = try_join!(
        ctx.archive()
            .fetch(inputs.registry, &inputs.package, &inputs.from_version),
        ctx.archive()
            .fetch(inputs.registry, &inputs.package, &inputs.to_version),
    )?;

    Ok(Versions {
        from_files,
        to_files,
    })
}

/// The comparison `handle` names.
///
/// The walk from a handle to a compared tree, in the module that owns the
/// comparison. It used to be written four times — here, in `get_diff_tree`,
/// in `get_file_diff` and in the `diffpack://diff/{handle}` resource — and
/// four copies of one walk is four places for the pair of downloads, the
/// rename threshold and the whitespace rule to stop agreeing. See [ADR
/// 0016](../../docs/adr/0016-the-walk-to-a-comparison-is-this-tools.md).
///
/// It is this module's rather than a shared one's because this is the tool
/// that computes a comparison: `diff_package_versions` is what mints a handle
/// and the other three read back what it worked out. A resource calling it is
/// the shape [ADR
/// 0014](../../docs/adr/0014-a-resource-is-a-projection-of-the-tools.md) asks
/// for, and it is one of this directory's exports to have a caller outside
/// the module that owns it.
///
/// # The store is on this side of it
///
/// The lookup used to be in the one caller that never needed it — the tool
/// that computes the comparison in the first place — so the three paths that
/// exist to *read* one each paid two downloads and a tree build for a tree
/// the store was already holding. It is here now, which is what makes a
/// cached comparison cheap for all four.
///
/// The write is here for the same reason and for one more: a miss is what
/// repairs an entry, so a reading path that recomputed and kept it to itself
/// would leave the next reader to pay again. What that costs a cold read is
/// the patches rendered below, which is a comparison of strings already in
/// memory — and what it buys is that eviction is a latency question rather
/// than a permanent one. See [ADR
/// 0006](../../docs/adr/0006-the-handle-carries-its-inputs.md).
///
/// Neither half of the store can fail this. [`crate::store::DiffStore`] has
/// no `Result` to return: a lookup that could not be made is a miss, and a
/// write that could not be made is a note in the log ([ADR
/// 0003](../../docs/adr/0003-the-cache-seam-is-a-store.md)). The `Failure`
/// here is the archive seam's and nothing else's.
pub async fn compare(handle: &DiffHandle, ctx: &Ctx) -> Result<Comparison, Failure> {
    let key = handle.key();

    // Everything below the cache is the same either way, because what is
    // remembered is the tree and not an answer: every caller walks what it
    // wants out of the tree, so a cached comparison and a fresh one cannot
    // differ without the tree differing.
    if let Some(entry) = ctx.store().get(&key).await {
        return Ok(Comparison {
            tree: entry.tree,
            cached: true,
            handle: handle.clone(),
            patches: entry.patches,
            files: None,
        });
    }

    let inputs = handle.inputs();
    let files = versions(handle, ctx).await?;

    let tree = engine::build_diff_tree(
        &files.from_files,
        &files.to_files,
        inputs.similarity_threshold,
        inputs.ignore_whitespace,
    );

    // Rendered now rather than by whoever asks for one later: both archives
    // are extracted at this moment, so a patch costs a comparison of two
    // strings already in memory — and on the other side of this call it
    // costs two downloads.
    let patches = rendered(&tree, &files, inputs.ignore_whitespace);

    ctx.store().put(Entry {
        key,
        tree: tree.clone(),
        patches,
    });

    Ok(Comparison {
        tree,
        cached: false,
        handle: handle.clone(),
        // Empty rather than the map above, which has just been handed to the
        // store. A caller holding both versions' files renders what it wants
        // from those, so a second copy of every changed file's patch would be
        // the largest thing here and read by nobody.
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

    /// The one tool here whose answer names something a client has no URI
    /// for yet. Every other tool either takes the handle this one minted or
    /// is not about a comparison at all, so a link on those would be the same
    /// URI repeated back at a caller already holding it.
    ///
    /// The URI is `crate::resources::diff`'s to spell. A tool that built one
    /// out of a handle would be the second place the format is written, and
    /// the first to be wrong when it moves.
    fn links(output: &Output) -> Vec<Resource> {
        vec![resources::diff::link(&output.handle)]
    }

    async fn call(args: Args, ctx: &Ctx) -> Result<Output, Failure> {
        let handle = DiffHandle::mint(Inputs {
            registry: args.registry,
            package: args.package,
            from_version: args.from_version,
            to_version: args.to_version,
            similarity_threshold: args.similarity_threshold,
            ignore_whitespace: args.ignore_whitespace,
        });
        let comparison = compare(&handle, ctx).await?;

        // The totals and the sample are walked out of the tree here, on both
        // paths, so a remembered answer and a fresh one cannot differ without
        // the tree differing.
        let mut totals = Totals::default();
        let mut changed = Vec::new();
        walk(&comparison.tree, &mut totals, &mut changed);

        // Most-moved first, and then by path. The second half is what makes
        // this an order rather than a tendency: a tree walk has its own
        // order, and two files with equal churn would otherwise swap places
        // between two calls that are supposed to be the same diff.
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
