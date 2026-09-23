//! `get_file_content` — one file out of one published version.
//!
//! The question an agent asks between reading a diff and reasoning about it:
//! what does this file actually say, before I decide what the change did to
//! it. [`super::list_package_files`] says what a version ships; this says
//! what one of those files is.
//!
//! # Three things the engine's behaviour forces into the description
//!
//! Each is a case where a correct answer reads as a wrong one to an agent
//! that was not told, which is why they are in [`Self::DESCRIPTION`] and not
//! only here.
//!
//! A **directory** has no content. The extractor gives one the empty string,
//! so passing that through would tell an agent that `src` is a file with
//! nothing in it — and an agent that concludes the package ships an empty
//! module has nothing in the answer to notice it with. It is refused
//! instead, on the channel the model reads, because the remedy is to ask for
//! a file inside. Which of the two a path is, is the FileMap's answer
//! ([`crate::archive::At`]) rather than something this tool reads off the
//! content.
//!
//! **Content is decoded lossily.** A file whose bytes are not UTF-8 arrives
//! as replacement characters rather than as a failure, which is the right
//! behaviour and useless on its own: a string of `U+FFFD` could be a PNG or
//! a source file saved in the wrong encoding, and those have different next
//! moves. `valid_utf8` is what separates them, and whether it holds is
//! asked of the file the FileMap answered with
//! ([`crate::archive::File::decoded_cleanly`]), which also says why it is
//! read off the whole file rather than off the excerpt.
//!
//! **A cut is loud.** A silently shortened file is how an agent concludes a
//! function does not exist: it read what it was given, found nothing, and
//! had no reason to doubt it. The cut, the marker and the real byte count
//! all come from [`crate::page`], which is also why the count is the decoded
//! text's size rather than the archive's — it is the size of the thing being
//! cut.
//!
//! # Where the descriptions come from
//!
//! Every doc comment on a field of [`Args`] and [`Content`] becomes a
//! `description` in a schema a model reads, so it is written for that reader
//! and names nothing in this repository. Why a field is shaped the way it is
//! belongs here or in an ordinary comment beside the code.
//!
//! `max_bytes` has no doc comment at all, deliberately, and for the reason
//! `cursor` and `limit` have none in [`super::list_package_files`]: it is
//! [`crate::page`]'s type and that module writes its description — the
//! range, and the rule that a value above the ceiling is narrowed rather
//! than refused. A doc comment here would *replace* that rather than add to
//! it, which is how a tool ends up telling an agent a number no test
//! compares against [`crate::page::PAYLOAD_CEILING`].
//!
//! The three fields of the answer that describe the cut come from that
//! module too, flattened into [`Content`] so that they are this tool's own
//! fields on the wire rather than a nested object an agent has to reach
//! into.

use serde::{Deserialize, Serialize};

use crate::archive::At;
use crate::error::Failure;
use crate::page::{self, Excerpt};
use crate::registry::Registry;
use crate::tools::{Call, Tool};

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
#[serde(rename_all = "camelCase")]
pub struct Content {
    // Flattened, so `text`, `truncated` and `bytes` are this answer's own
    // fields rather than a nested object. The three of them and the sentences
    // that describe them belong to the module that does the cutting; what
    // this tool adds is the field below.
    #[serde(flatten)]
    pub excerpt: Excerpt,

    /// False when the text you were given is not what the file holds: its
    /// bytes were not valid UTF-8, and each one that could not be decoded is
    /// shown as the `U+FFFD` replacement character. A binary file therefore
    /// reads as mostly replacement characters instead of failing. True means
    /// every byte decoded, so the text is the file.
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

    async fn call(args: Args, call: &Call) -> Result<Content, Failure> {
        let files = call
            .archive()
            .fetch(args.registry, &args.package, &args.version)
            .await?;

        // The FileMap says what is at the path, and this is where the two
        // answers that are not a file are refused. A directory is refused
        // rather than read as empty, and the FileMap's answer is what keeps a
        // genuinely empty file answering as one.
        match files.at(&args.path) {
            At::Nothing => Err(Failure::NoSuchFile {
                package: args.package,
                version: args.version,
                path: args.path,
            }),

            At::Directory => Err(Failure::PathIsDirectory {
                package: args.package,
                version: args.version,
                path: args.path,
            }),

            At::File(file) => Ok(Content {
                excerpt: page::truncate(file.text(), args.max_bytes),
                valid_utf8: file.decoded_cleanly(),
            }),
        }
    }
}
