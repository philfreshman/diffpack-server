use serde::{Deserialize, Serialize};

use crate::archive::At;
use crate::error::Failure;
use crate::page::{self, Excerpt};
use crate::registry::{PackageName, Registry, VersionName};
use crate::tools::{Call, Tool};

pub struct GetFileContent;

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[schemars(description = "What a caller asks for.")]
#[serde(deny_unknown_fields)]
pub struct Args {
    #[schemars(description = "The registry that publishes the package.")]
    pub registry: Registry,

    pub package: PackageName,

    pub version: VersionName,

    #[schemars(
        description = "Where the file is inside the archive, with the archive's top-level\n\
        directory already removed: `src/index.js`, never\n\
        `zod-4.0.0/src/index.js`."
    )]
    pub path: String,

    #[serde(default)]
    pub max_bytes: Option<page::MaxBytes>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, schemars::JsonSchema)]
#[schemars(description = "One file, as much of it as fits.")]
#[serde(rename_all = "camelCase")]
pub struct Content {
    #[serde(flatten)]
    pub excerpt: Excerpt,

    #[schemars(
        description = "False when the text you were given is not what the file holds: its\n\
        bytes were not valid UTF-8, and each one that could not be decoded is\n\
        shown as the `U+FFFD` replacement character. A binary file therefore\n\
        reads as mostly replacement characters instead of failing. True means\n\
        every byte decoded, so the text is the file."
    )]
    pub valid_utf8: bool,
}

impl Tool for GetFileContent {
    const NAME: &'static str = "get_file_content";
    const TITLE: &'static str = "Get file content";
    const DESCRIPTION: &'static str = "\
        Read one file out of one published version of a package, so you can \
        see what it actually says before reasoning about a change to it. \
        Takes a registry, a package name, one exact version and a path. \
        Paths have the archive's top-level directory removed: a file is \
        `src/index.js`, never `zod-4.0.0/src/index.js`. A directory has no \
        content, so asking for one is an error rather than an empty file. A \
        file whose bytes are not valid UTF-8 is decoded anyway, with each \
        byte that could not be decoded shown as `U+FFFD`; the answer says \
        when that happened, so replacement characters do not mean the file \
        arrived broken. A long file is cut short, and the answer says so and \
        gives the whole file's size in bytes — which is the decoded text's \
        size, not the archive's.";

    const READ_ONLY: bool = true;

    const IDEMPOTENT: bool = true;

    const OPEN_WORLD: bool = true;

    type Args = Args;
    type Output = Content;

    async fn call(args: Args, call: &Call) -> Result<Content, Failure> {
        let files = call
            .archive()
            .fetch(args.registry, args.package.as_str(), args.version.as_str())
            .await?;

        match files.at(&args.path) {
            At::Nothing => Err(Failure::NoSuchFile {
                package: args.package.into(),
                version: args.version.into(),
                path: args.path,
            }),

            At::Directory => Err(Failure::PathIsDirectory {
                package: args.package.into(),
                version: args.version.into(),
                path: args.path,
            }),

            At::File(file) => Ok(Content {
                excerpt: page::truncate(file.text(), args.max_bytes),
                valid_utf8: file.decoded_cleanly(),
            }),
        }
    }
}
