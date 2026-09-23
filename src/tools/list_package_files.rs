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
//! path it is shown, which is why the description says it out loud.
//!
//! # Where the descriptions come from
//!
//! Every doc comment on a field of [`Args`], [`Entry`] and [`EntryType`]
//! becomes a `description` in a schema a model reads, so it is written for
//! that reader and names nothing in this repository. Why a field is shaped
//! the way it is belongs here or in an ordinary comment beside the code.
//!
//! Three fields have no doc comment at all, deliberately. `prefix`, `cursor`
//! and `limit` are [`crate::page`]'s types and that module writes their
//! descriptions — what a directory's subtree is and is not, the default, the
//! range, and the rule that an out-of-range `limit` is clamped rather than
//! refused. A doc comment here would *replace* those rather than add to them,
//! which is how a tool ends up telling an agent numbers no test compares
//! against `page::MAX_LIMIT` — and how two tools that take a directory ended
//! up with two rules for one, which disagreed about `/` (#97).

use serde::{Deserialize, Serialize};

use crate::archive::At;
use crate::error::Failure;
use crate::page::{self, Page};
use crate::registry::Registry;
use crate::tools::{Call, Tool};

/// The tool.
pub struct ListPackageFiles;

/// What a caller asks for.
///
/// Nothing is normalised: the package name and the version go to the
/// registry exactly as they arrive, the rule `docs/cache-key.md` fixes for
/// the cache key and [`super::resolve_archive_url`] follows for the same
/// reason. The doc comments below are read by a model — see the module
/// header.
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

    /// The version as the registry spells it: `4.0.0`. Not a range, not a
    /// tag — one published version.
    pub version: String,

    // No doc comment on this or the two below, on purpose: see the module
    // header. `page` writes their descriptions — this one the rule for a
    // directory whose subtree is asked for, which `get_diff_tree` takes as
    // well, and the other two the numbers that bind — and a sentence here
    // would replace them.
    #[serde(default)]
    pub prefix: Option<page::Subtree>,

    #[serde(default)]
    pub cursor: Option<page::Cursor>,

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

/// A file, or a directory holding other entries.
// Written here rather than taken from the archive seam because a tool
// declares types, not JSON, and `archive::At` carries no JSON schema. The two
// variants are the two of its three answers that are something to list, and
// `Entry::at` below matches all three, so a fourth answer in the FileMap is a
// compile error here rather than a value this server quietly renames.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum EntryType {
    File,
    Directory,
}

impl Entry {
    /// The entry for `path`, given what the FileMap has at it, or nothing
    /// where it has nothing.
    fn at(path: &str, at: At<'_>) -> Option<Self> {
        let (entry_type, size) = match at {
            At::File(file) => (EntryType::File, file.text().len()),
            // A directory has no text, so it has no size. Stated rather than
            // taken from the content the extractor gave it, which happens to
            // be empty and is not this tool's to read.
            At::Directory => (EntryType::Directory, 0),
            At::Nothing => return None,
        };

        Some(Self {
            path: path.to_owned(),
            entry_type,
            size,
        })
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

    async fn call(args: Args, call: &Call) -> Result<Page<Entry>, Failure> {
        let files = call
            .archive()
            .fetch(args.registry, &args.package, &args.version)
            .await?;

        // An absent prefix is the root, which is what `/` is: the whole
        // archive. The Subtree has already normalised what was asked for,
        // and it is what knows that `sr` is not `src`.
        let under = args.prefix.unwrap_or_default();

        // In the FileMap's order, which is the paths' own. A listing needs
        // one that holds from call to call, or a cursor names a position in
        // an arrangement that existed for one request.
        let entries: Vec<Entry> = files
            .paths()
            .into_iter()
            .filter(|path| under.contains(path))
            .filter_map(|path| Entry::at(path, files.at(path)))
            .collect();

        page::paginate(entries, args.limit, args.cursor)
    }
}
