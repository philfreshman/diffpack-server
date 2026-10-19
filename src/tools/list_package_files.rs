use serde::{Deserialize, Serialize};

use crate::archive::At;
use crate::error::Failure;
use crate::page::{self, Page};
use crate::registry::{PackageName, Registry, VersionName};
use crate::tools::{Call, Tool};

pub struct ListPackageFiles;

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[schemars(
    description = "What a caller asks for.\n\
    \n\
    Nothing is normalised: the package name and the version go to the\n\
    registry exactly as they arrive, the rule `docs/cache-key.md` fixes for\n\
    the cache key and [`super::resolve_archive_url`] follows for the same\n\
    reason. The doc comments below are read by a model — see the module\n\
    header."
)]
#[serde(deny_unknown_fields)]
pub struct Args {
    #[schemars(description = "The registry that publishes the package.")]
    pub registry: Registry,

    pub package: PackageName,

    pub version: VersionName,

    #[serde(default)]
    pub prefix: Option<page::Subtree>,

    #[serde(default)]
    pub cursor: Option<page::Cursor>,

    #[serde(default)]
    pub limit: Option<page::Limit>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, schemars::JsonSchema)]
#[schemars(description = "One thing inside the archive.")]
pub struct Entry {
    #[schemars(description = "Where it is, with the archive's top-level directory stripped.")]
    pub path: String,

    #[schemars(
        description = "Whether this is a file or a directory.\n\
        \n\
        The part a path does not carry: `src` and `src/lib.rs` look alike and\n\
        only one of them has content to ask for."
    )]
    #[serde(rename = "type")]
    pub entry_type: EntryType,

    #[schemars(
        description = "How many bytes the file's extracted text is. Zero for a directory.\n\
        \n\
        The extracted text, not the bytes the registry served: the archive is\n\
        decompressed before anything is counted, and a file that is not valid\n\
        UTF-8 is read lossily, so its size here is the readable form's. It is\n\
        what deciding whether to ask for the content costs, which is the\n\
        question this field is for."
    )]
    pub size: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, schemars::JsonSchema)]
#[schemars(description = "A file, or a directory holding other entries.")]
#[serde(rename_all = "lowercase")]
pub enum EntryType {
    File,
    Directory,
}

impl Entry {
    fn at(path: &str, at: At<'_>) -> Option<Self> {
        let (entry_type, size) = match at {
            At::File(file) => (EntryType::File, file.text().len()),
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
        see what it actually ships. To see which files changed between two \
        versions, compare them with `diff_package_versions` and walk the \
        result with `get_diff_tree` instead: this lists one version as it \
        stands. For the address a version's archive is served from rather \
        than what is in it, call `resolve_archive_url`. Takes a registry, a \
        package name and one \
        exact version, all spelled the way the registry spells them. Paths \
        have the archive's top-level directory removed: a file is \
        `src/index.js`, never `zod-4.0.0/src/index.js`.";

    const READ_ONLY: bool = true;

    const IDEMPOTENT: bool = true;

    const OPEN_WORLD: bool = true;

    type Args = Args;
    type Output = Page<Entry>;

    async fn call(args: Args, call: &Call) -> Result<Page<Entry>, Failure> {
        let files = call
            .archive()
            .fetch(args.registry, args.package.as_str(), args.version.as_str())
            .await?;

        let under = args.prefix.unwrap_or_default();

        let entries: Vec<Entry> = files
            .paths()
            .into_iter()
            .filter(|path| under.contains(path))
            .filter_map(|path| Entry::at(path, files.at(path)))
            .collect();

        page::paginate(entries, args.limit, args.cursor)
    }
}
