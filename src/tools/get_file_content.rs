//! `get_file_content` — one file out of one published version.

use serde::{Deserialize, Serialize};

use crate::engine;
use crate::error::Failure;
use crate::page::{self, Excerpt};
use crate::registry::Registry;
use crate::tools::{Ctx, Tool};

/// The tool.
pub struct GetFileContent;

/// What a caller asks for.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Args {
    /// The registry that publishes the package.
    pub registry: Registry,

    /// The package name as the registry spells it, scope included:
    /// `zod`, `@types/node`, `serde`.
    pub package: String,

    /// The version as the registry spells it: `4.0.0`. Not a range, not a
    /// tag — one published version.
    pub version: String,

    /// Where the file is inside the archive, with the archive's top-level
    /// directory already removed: `src/index.js`, never
    /// `zod-4.0.0/src/index.js`.
    pub path: String,

    // No doc comment, on purpose: `page` writes this field's description,
    // and a sentence here would replace the one carrying the number that
    // binds. The same rule the paging arguments follow.
    #[serde(default)]
    pub max_bytes: Option<page::MaxBytes>,
}

/// One file, as much of it as fits.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, schemars::JsonSchema)]
pub struct Content {
    #[serde(flatten)]
    pub excerpt: Excerpt,
}

impl Tool for GetFileContent {
    const NAME: &'static str = "get_file_content";
    const TITLE: &'static str = "Get file content";
    const DESCRIPTION: &'static str = "\
        Read one file out of one published version of a package. Takes a \
        registry, a package name, one exact version and a path. Paths have \
        the archive's top-level directory removed: a file is \
        `src/index.js`, never `zod-4.0.0/src/index.js`.";

    /// It downloads and reads; it changes nothing anywhere.
    const READ_ONLY: bool = true;

    /// A published version's contents are immutable, so the same arguments
    /// always give the same file.
    const IDEMPOTENT: bool = true;

    /// The arguments name a package on a registry, which is a world this
    /// server does not control.
    const OPEN_WORLD: bool = true;

    type Args = Args;
    type Output = Content;

    async fn call(args: Args, ctx: &Ctx) -> Result<Content, Failure> {
        let files = ctx
            .archive()
            .fetch(args.registry, &args.package, &args.version)
            .await?;

        let Some(entry) = files.get(&args.path) else {
            return Err(Failure::NoSuchFile {
                package: args.package,
                version: args.version,
                path: args.path,
            });
        };

        // A directory's content is the empty string the extractor gave it, so
        // the type is the only thing that tells the two apart. Reading the
        // type rather than the emptiness is what keeps a genuinely empty file
        // answering as one.
        if matches!(entry.file_type, engine::FileType::Directory) {
            return Err(Failure::PathIsDirectory {
                package: args.package,
                version: args.version,
                path: args.path,
            });
        }

        Ok(Content {
            excerpt: page::truncate(&entry.content, args.max_bytes),
        })
    }
}
