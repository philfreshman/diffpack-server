//! `get_file_diff` — one file's patch out of a comparison already made.
//!
//! What an agent is actually after once `get_diff_tree` has told it which
//! files moved: the unified diff for one of them.

use std::borrow::Cow;

use futures::try_join;
use schemars::{json_schema, JsonSchema, Schema, SchemaGenerator};
use serde::{de, Deserialize, Deserializer, Serialize};

use crate::archive::FileMap;
use crate::engine::{self, FileType};
use crate::error::Failure;
use crate::handle::DiffHandle;
use crate::page::{self, Excerpt};
use crate::tools::{Ctx, Tool};

/// The tool.
pub struct GetFileDiff;

/// What a caller asks for.
///
/// The doc comments below are read by a model — see [`super`].
#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Args {
    // No doc comment, on purpose: the handle writes its own description,
    // which is the same one the tool that minted it shows in its answer.
    pub handle: DiffHandle,

    /// Where the file is in the second version, with the archive's top-level
    /// directory already removed: `src/index.js`, never
    /// `zod-4.0.0/src/index.js`. For a file the second version does not have,
    /// where it was in the first.
    pub path: String,

    /// Where the file was in the first version, when it moved. Pass the
    /// `old_path` the comparison gave you for a renamed file; omit it for
    /// everything else.
    #[serde(default)]
    pub old_path: Option<String>,

    // No doc comment: the type writes its own, for the reason `page`'s types
    // do. See `ContextLines`.
    #[serde(default)]
    pub context_lines: Option<ContextLines>,

    // No doc comment, on purpose: `page` writes this field's description,
    // and a sentence here would replace the one carrying the number that
    // binds.
    #[serde(default)]
    pub max_bytes: Option<page::MaxBytes>,
}

/// One file's patch, as much of it as fits.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Patch {
    // Flattened, so `text`, `truncated` and `bytes` are this answer's own
    // fields rather than a nested object, the way `get_file_content` carries
    // them.
    #[serde(flatten)]
    pub excerpt: Excerpt,

    /// True when `text` is a diff — a `--- from` / `+++ to` header and one
    /// line per change. False when it is not, and then it is the file's own
    /// content: a file both versions have byte for byte has no diff to show,
    /// and a path neither version has is a sentence saying so. Render a
    /// `false` as a file rather than trying to parse it as a patch.
    pub is_diff: bool,
}

/// How much unchanged context to keep around each change.
///
/// This tool's own type rather than [`crate::page`]'s, because the rule it
/// carries is this tool's own: the engine emits every line of a file,
/// including all of the unchanged ones, because the browser renders the whole
/// file in a scrollable view. Trimming that down is presentation over the
/// engine's output and not a change to how a diff is computed — so the
/// argument, its default and the word that turns it off belong here, with the
/// code that does the trimming.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContextLines {
    /// Keep this many unchanged lines either side of each change.
    Around(u32),

    /// Keep the engine's output exactly as it is.
    Full,
}

/// How much context a caller gets when they do not say.
///
/// Git's default, and the number a person reads a patch at. Handing an agent
/// four thousand lines to communicate a three-line change spends its context
/// and this server's response budget on what did not happen.
pub const DEFAULT_CONTEXT: u32 = 3;

/// The word that turns trimming off.
const FULL: &str = "full";

/// What the engine answers about a path in neither version, word for word.
const ABSENT: &str = "File not present in either version.";

impl<'de> Deserialize<'de> for ContextLines {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        match Asked::deserialize(deserializer)? {
            Asked::Around(lines) => Ok(Self::Around(lines)),
            Asked::Word(word) if word == FULL => Ok(Self::Full),
            Asked::Word(word) => Err(de::Error::custom(format!(
                "`{word}` is not a context setting. Pass a number of lines, or `{FULL}` \
                 for every line of the file."
            ))),
        }
    }
}

/// The two shapes this argument arrives in.
///
/// Untagged rather than a hand-written visitor: what a caller writes is a
/// number or a word, and the refusal for anything else is written above
/// rather than left to `serde`'s "did not match any variant".
#[derive(Deserialize)]
#[serde(untagged)]
enum Asked {
    Around(u32),
    Word(String),
}

/// The default and the word, where an agent reads them.
impl JsonSchema for ContextLines {
    fn schema_name() -> Cow<'static, str> {
        "ContextLines".into()
    }

    fn schema_id() -> Cow<'static, str> {
        concat!(module_path!(), "::ContextLines").into()
    }

    /// Inline rather than a `$ref`, for the reason [`crate::page`]'s types
    /// are: the reader is a model deciding what to pass, and a rule it has to
    /// resolve a reference to learn is one it will guess at instead.
    fn inline_schema() -> bool {
        true
    }

    fn json_schema(_generator: &mut SchemaGenerator) -> Schema {
        json_schema!({
            "oneOf": [
                { "type": "integer", "minimum": 0 },
                { "const": FULL },
            ],
            "default": DEFAULT_CONTEXT,
            "description": format!(
                "How many unchanged lines to keep either side of each change. Omit it \
                 for {DEFAULT_CONTEXT}, which is what a patch is normally read at. Pass \
                 `{FULL}` for every line of the file, which is what a viewer showing the \
                 whole file in a scrollable pane needs and is almost never what you want \
                 — a package's files run to thousands of lines and the change in one is \
                 usually three.",
            ),
        })
    }
}

/// One file's patch, and whether what came back is a diff at all.
///
/// The four-case renderer `diffpack-engine`'s `build_diff_result` is, written
/// out here because that function is private to the engine and
/// `get_diff_for_path` beside it is a `wasm_bindgen` entry point. The output
/// has to stay byte-identical to what the browser shows, so this is a
/// transcription rather than an implementation: every wart below is the
/// engine's, including the trailing `+ ` a file ending in a newline gets from
/// splitting on `\n`.
fn render(path: &str, from: Option<&str>, to: Option<&str>, ignore_whitespace: bool) -> (String, bool) {
    match (from, to) {
        (None, None) => (ABSENT.to_owned(), false),

        (None, Some(to)) => (sided(&format!("--- /dev/null\n+++ to/{path}"), '+', to), true),

        (Some(from), None) => (
            sided(&format!("--- from/{path}\n+++ /dev/null"), '-', from),
            true,
        ),

        // Byte equality, and deliberately not the comparison
        // `ignore_whitespace` would make: a file that was reformatted *did*
        // change, and answering with its content would hide the reformatting
        // that is the only thing that happened to it. The engine draws the
        // line here too.
        (Some(from), Some(to)) if from == to => (to.to_owned(), false),

        (from, to) => (
            engine::get_diff_content(
                path,
                from.unwrap_or_default(),
                to.unwrap_or_default(),
                ignore_whitespace,
            ),
            true,
        ),
    }
}

/// `header`, then every line of `content` under `sign`.
///
/// The engine splits on `\n` here rather than diffing, which is what gives a
/// file ending in a newline one more line than it has.
fn sided(header: &str, sign: char, content: &str) -> String {
    let mut text = header.to_owned();
    for line in content.split('\n') {
        text.push('\n');
        text.push(sign);
        text.push(' ');
        text.push_str(line);
    }
    text
}

/// The content of `path` in `files`, or nothing when the version has no file
/// there.
///
/// A directory is nothing, which is the engine's reading and the reason the
/// two are separated before this is called: a directory's content is the
/// empty string the extractor gave it, so a caller that named one would
/// otherwise be told the file is in neither version.
fn content<'f>(files: &'f FileMap, path: &str) -> Option<&'f str> {
    files.get(path).and_then(|entry| match entry.file_type {
        FileType::File => Some(entry.content.as_str()),
        FileType::Directory => None,
    })
}

impl Tool for GetFileDiff {
    const NAME: &'static str = "get_file_diff";
    const TITLE: &'static str = "Get file diff";
    const DESCRIPTION: &'static str = "\
        Read the unified diff for one file of a comparison you have already \
        made. Takes the handle `diff_package_versions` gave you and a path \
        from the comparison's tree.";

    /// It downloads and compares; it changes nothing anywhere.
    const READ_ONLY: bool = true;

    /// A handle names two published versions, which are immutable, so the
    /// same arguments always give the same patch.
    const IDEMPOTENT: bool = true;

    /// The handle names a package on a registry, which is a world this
    /// server does not control.
    const OPEN_WORLD: bool = true;

    type Args = Args;
    type Output = Patch;

    async fn call(args: Args, ctx: &Ctx) -> Result<Patch, Failure> {
        let inputs = args.handle.inputs();

        // Concurrently, the way the tool that minted this handle fetched
        // them: the two downloads do not depend on each other.
        let (from_files, to_files) = try_join!(
            ctx.archive()
                .fetch(inputs.registry, &inputs.package, &inputs.from_version),
            ctx.archive()
                .fetch(inputs.registry, &inputs.package, &inputs.to_version),
        )?;

        let from_path = args.old_path.as_deref().unwrap_or(&args.path);
        let from = content(&from_files, from_path);
        let to = content(&to_files, &args.path);

        let (text, is_diff) = render(&args.path, from, to, inputs.ignore_whitespace);

        Ok(Patch {
            excerpt: page::truncate(&text, args.max_bytes),
            is_diff,
        })
    }
}
