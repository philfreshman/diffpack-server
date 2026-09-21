//! `list_package_files` — what one published version ships.
//!
//! The first tool that reads a package rather than reasoning about its name,
//! and the cheapest end-to-end proof this server has: resolution, download,
//! extraction and the MCP layer all have to work for it to answer at all.
//!
//! # The paths it answers with
//!
//! The archive's top-level directory is already gone — `src/index.js`, not
//! `zod-4.0.0/src/index.js`. That is [`crate::archive`]'s doing rather than
//! this tool's, and it is the property that makes two versions comparable in
//! the first place. It is also the one thing an agent cannot infer from a
//! path it is shown, which is why [`Self::DESCRIPTION`] says it out loud.

use serde::{Deserialize, Serialize};

use crate::engine;
use crate::error::Failure;
use crate::page::{self, Page};
use crate::registry::Registry;
use crate::tools::{Ctx, Tool};

/// The tool.
pub struct ListPackageFiles;

/// What a caller asks for.
///
/// Nothing is normalised: the package name and the version go to the
/// registry exactly as they arrive, the rule `docs/cache-key.md` fixes for
/// the cache key and [`super::resolve_archive_url`] follows for the same
/// reason.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Args {
    /// The registry that publishes the package.
    ///
    /// The enum comes from [`crate::registry`], so the list an agent is shown
    /// is the list this server has rather than a description of it.
    pub registry: Registry,

    /// The package name as the registry spells it, scope included:
    /// `zod`, `@types/node`, `serde`.
    pub package: String,

    /// The version as the registry spells it: `4.0.0`. Not a range, not a
    /// tag — one published version.
    pub version: String,

    /// Only what is inside this directory, one level or many: `src`, or
    /// `src/util`. A trailing slash is allowed and makes no difference.
    ///
    /// It names a directory and is not matched by characters, so `sr` does
    /// not narrow to `src/`. The directory itself is not in its own subtree.
    /// Omit it for the whole archive. A directory the version does not have
    /// is an empty page rather than an error.
    #[serde(default)]
    pub prefix: Option<String>,

    /// Where to resume: the `nextCursor` from the previous page, unchanged.
    ///
    /// Declared as [`crate::page`]'s type, so the rule an agent reads — that
    /// a cursor is opaque and one written by hand is refused — is in the
    /// schema that module owns rather than in a sentence this tool wrote.
    #[serde(default)]
    pub cursor: Option<page::Cursor>,

    /// How many entries this page holds.
    ///
    /// The default and the range are [`crate::page`]'s too, for the same
    /// reason. A page can still come back shorter than the limit: the
    /// response ceiling is what actually bounds an answer.
    #[serde(default)]
    pub limit: Option<page::Limit>,
}

/// One thing inside the archive.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, schemars::JsonSchema)]
pub struct Entry {
    /// Where it is, with the archive's top-level directory stripped.
    pub path: String,

    /// Whether this is a file or a directory.
    ///
    /// The part a path does not carry: `src` and `src/lib.rs` look alike and
    /// only one of them has content to ask for.
    #[serde(rename = "type")]
    pub entry_type: EntryType,

    /// How many bytes the file's extracted text is. Zero for a directory.
    ///
    /// The extracted text, not the bytes the registry served: the archive is
    /// decompressed before anything is counted, and a file that is not valid
    /// UTF-8 is read lossily, so its size here is the readable form's. It is
    /// what deciding whether to ask for the content costs, which is the
    /// question this field is for.
    pub size: usize,
}

/// What an [`Entry`] is.
///
/// Two variants that mirror [`engine::FileType`], written here rather than
/// re-exported because the engine's type carries no JSON schema and a tool
/// declares types, not JSON. The [`From`] below is a total `match`, so a
/// third variant in the engine is a compile error here rather than a value
/// this server quietly renames.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum EntryType {
    File,
    Directory,
}

impl From<engine::FileType> for EntryType {
    fn from(file_type: engine::FileType) -> Self {
        match file_type {
            engine::FileType::File => Self::File,
            engine::FileType::Directory => Self::Directory,
        }
    }
}

impl Tool for ListPackageFiles {
    const NAME: &'static str = "list_package_files";
    const TITLE: &'static str = "List package files";
    const DESCRIPTION: &'static str = "\
        List the files inside one published version of a package, so you can \
        see what it actually ships. Takes a registry, a package name and one \
        exact version, all spelled the way the registry spells them. Paths \
        have the archive's top-level directory removed: a file is \
        `src/index.js`, never `zod-4.0.0/src/index.js`.";

    /// It downloads and reads; it changes nothing anywhere.
    const READ_ONLY: bool = true;

    /// A published version's contents are immutable, so the same arguments
    /// always give the same listing.
    const IDEMPOTENT: bool = true;

    /// The arguments name a package on a registry, which is a world this
    /// server does not control.
    const OPEN_WORLD: bool = true;

    type Args = Args;
    type Output = Page<Entry>;

    async fn call(args: Args, ctx: &Ctx) -> Result<Page<Entry>, Failure> {
        let files = ctx
            .archive()
            .fetch(args.registry, &args.package, &args.version)
            .await?;

        // A `FileMap` is a map, so it has no order of its own and two calls
        // would not agree on one. Sorting is what makes the sequence a
        // sequence — without it a cursor names a position in an arrangement
        // that existed for one request.
        // A directory, with the slash a caller may or may not have written
        // put back exactly once. Matching against `src/` rather than `src`
        // is the whole of what makes this a directory and not a string
        // comparison: `sr` cannot be a prefix of `src/lib.rs`, and `lib`
        // cannot swallow `libs/`.
        let under = args
            .prefix
            .as_deref()
            .map(|prefix| format!("{}/", prefix.trim_end_matches('/')));

        let mut entries: Vec<Entry> = files
            .into_iter()
            .filter(|(path, _)| match &under {
                Some(under) => path.starts_with(under),
                None => true,
            })
            .map(|(path, entry)| Entry {
                path,
                entry_type: entry.file_type.into(),
                // A directory's content is the empty string the extractor
                // gave it, so this is already zero for one; taking the
                // length rather than matching on the type keeps the two
                // fields from being able to disagree.
                size: entry.content.len(),
            })
            .collect::<Vec<_>>();
        entries.sort_by(|a, b| a.path.cmp(&b.path));

        page::paginate(entries, args.limit, args.cursor)
    }
}
