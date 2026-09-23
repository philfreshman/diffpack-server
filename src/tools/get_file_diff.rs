//! `get_file_diff` — one file's patch out of a comparison already made.
//!
//! The question an agent asks after [`super::get_diff_tree`] has told it
//! which files moved: what did this one actually change. That tool says a
//! file is `modified` and how many lines it gained and lost; this is the
//! lines.
//!
//! # The renderer is the engine seam's
//!
//! `diffpack-engine` decides which of five things one file's answer is, in a
//! function that is private to it and reachable only through a
//! `wasm_bindgen` entry point — so it is written out rather than called, and
//! [ADR 0013](../../docs/adr/0013-the-patch-renderer-lives-in-the-engine-seam.md)
//! puts the transcription in [`crate::engine`] rather than here. This tool
//! and the one that fills the cache render the same file, and two
//! transcriptions that drifted would answer it differently depending on
//! whether anyone had asked for it before.
//!
//! Nor is the rendering of one file this module's. Which patch answers a
//! file — the one a remembered comparison holds, or one rendered out of both
//! versions — is
//! [`Comparison::file_patch`](super::diff_package_versions::Comparison::file_patch)'s,
//! beside the pre-render that fills the cache, so the two read a file through
//! one lookup rather than two copies of it. What stays here is what the engine
//! has no opinion about: the trimming two sections down, and the cut.
//!
//! Only the fourth case — both versions have the file and it changed — goes
//! through anything public, so it is the only one `tests/get_file_diff.rs`
//! can hold against the engine directly. The rest are held against #15's
//! table.
//!
//! # One departure, on purpose
//!
//! A directory is refused. The engine reads one as having no content, which
//! puts it in the fifth case and answers that a directory both versions ship
//! is in neither of them. That is false about a path the package has and an
//! agent has nothing in the answer to doubt it with, so it takes the
//! refusal [`super::get_file_content`] gives a directory, for the same
//! reason. The refusal is made where the rendering is: out of the tree before
//! anything is downloaded, and out of the file maps for the one directory the
//! tree does not have, the one a rename emptied.
//!
//! # Why `context_lines` is this tool's and not the engine's
//!
//! The engine emits every line of a file, unchanged ones included, because
//! the browser renders the whole file in a scrollable pane. An agent reading
//! a three-line change does not want the other three thousand nine hundred,
//! so [`trim`] cuts them here — and adds the `@@` headers the engine has no
//! need for, because once lines are missing there is no other way to know
//! where the rest of them sat.
//!
//! That is presentation over the engine's output and not a second way of
//! computing a diff, which is a property rather than an intention: every line
//! a trimmed answer carries is a line of the full one, in the full one's
//! order. The suite holds it over six files rather than reading it off the
//! implementation.
//!
//! # Where a cached result comes in
//!
//! The walk from a handle to a comparison is
//! [`super::diff_package_versions::compare`]'s (#83), and it looks in the
//! store before it looks at a registry. What comes back carries one of two
//! halves beside the tree, and both of them can answer this tool.
//!
//! Which one answers is not this tool's to decide. It asks
//! [`super::diff_package_versions::Comparison::file_patch`] for the one file
//! it is about and is handed the patch: the one the entry was written with,
//! where the comparison was remembered with one for this file, and a warm call
//! that gets one fetches nothing at all. Otherwise both versions' files — the
//! ones this very call downloaded, or two downloads now — and the file
//! rendered out of them. That covers one over the store's per-patch cap, one
//! dropped with the rest because the entry was too big, one in an entry
//! written before there were patches to write, and a file that did not change
//! and so never had a patch at all. Every one of those is a file to render,
//! which is why the question is "does this comparison hold this file's patch"
//! and never "did this entry keep its patches".
//!
//! A directory is answered before either, out of the tree, which already says
//! what it is. It has no patch to find and nothing to render, so two
//! downloads to learn it again would be spent on nothing.
//!
//! On a cold call this tool pays more than it did before #83, and on
//! purpose. It used to fetch two archives and render one file and build no
//! tree; the walk it now calls builds the tree, renders every changed file
//! and writes the entry. That is spent on the call after it — here and on
//! the three other paths — rather than downloading two archives and
//! forgetting them.
//!
//! # Where the descriptions come from
//!
//! Every doc comment on a field of [`Args`] and [`Patch`] becomes a
//! `description` in a schema a model reads, so it is written for that reader
//! and names nothing in this repository. Why a field is shaped the way it is
//! belongs here or in an ordinary comment beside the code.
//!
//! Two fields have no doc comment at all, deliberately. `handle` is
//! [`crate::handle`]'s type and `max_bytes` is [`crate::page`]'s, and those
//! modules write their descriptions. A doc comment here would *replace*
//! those rather than add to them, which is how four tools that take one
//! handle end up describing it four ways. [`ContextLines`] is this module's
//! own type and writes its own for the same reason: the rule is this tool's,
//! so the schema carrying it is too.

use std::borrow::Cow;

use schemars::{json_schema, JsonSchema, Schema, SchemaGenerator};
use serde::{de, Deserialize, Deserializer, Serialize};

use crate::engine;
use crate::error::Failure;
use crate::handle::DiffHandle;
use crate::page::{self, Excerpt};
use crate::tools::{diff_package_versions, Ctx, Tool};

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
    //
    // Not an `Option`, so that the schema a model reads is the two shapes
    // this argument takes rather than those wrapped in an `anyOf` with a
    // null beside them. Absent means the default, which the type answers for.
    #[serde(default)]
    pub context_lines: ContextLines,

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

/// An absent argument is [`DEFAULT_CONTEXT`], here rather than at the point
/// of use so that the number in the schema and the number the handler applies
/// are one constant.
impl Default for ContextLines {
    fn default() -> Self {
        Self::Around(DEFAULT_CONTEXT)
    }
}

/// The word that turns trimming off.
const FULL: &str = "full";

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
/// number or a word, and the refusal for a word that is not [`FULL`] is
/// written above rather than left to `serde`'s "did not match any variant" —
/// which is still what a value that is neither shape gets.
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

/// What one line of a rendered diff is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    /// A line both versions have. The only kind trimming drops.
    Context,
    /// A line the first version has and the second does not.
    Removed,
    /// A line the second version has and the first does not.
    Added,
}

impl Kind {
    /// The kind `line` carries in its first character.
    ///
    /// Anything that is not a `-` or a `+` is context. The engine writes a
    /// space there, and reading an unexpected character as context is the
    /// fail-safe direction: a line trimming does not understand is kept and
    /// counted on both sides rather than dropped.
    fn of(line: &str) -> Self {
        match line.as_bytes().first() {
            Some(b'-') => Self::Removed,
            Some(b'+') => Self::Added,
            _ => Self::Context,
        }
    }
}

/// One line of a rendered diff, and where it sits in each of the two files.
///
/// A context line occupies a position in both, a removed line only in the
/// first and an added line only in the second — which is the whole of what a
/// hunk header counts.
#[derive(Debug, Clone, Copy)]
struct Placed {
    kind: Kind,
    /// Its line number in the first version, where that version has it.
    first: Option<usize>,
    /// Its line number in the second version, where that version has it.
    second: Option<usize>,
}

/// Which side of a [`Placed`] a hunk header is being written for.
type Side = fn(&Placed) -> Option<usize>;

/// `text`, cut down to `context` unchanged lines either side of each change.
///
/// The engine emits every line of a file, because the browser renders the
/// whole file in a scrollable pane. An agent reading a three-line change out
/// of a four-thousand-line file does not want the other three thousand nine
/// hundred, so they are dropped here — and the `@@` headers the engine has no
/// need for are added, because once lines are missing a caller has no other
/// way to know where the rest of them sat.
///
/// Nothing is rewritten. Every line this returns that is not a header is a
/// line of `text`, in `text`'s order, which is what makes the trimmed answer
/// derivable from the full one rather than a second rendering of the same
/// diff.
fn trim(text: &str, context: u32) -> String {
    let context = context as usize;

    // Split rather than `lines`, which would also strip a `\r`. The engine
    // keeps one, on the grounds that it belongs to the line, and a trim that
    // quietly dropped it would not be a subset of what it was given.
    let mut all = text.split('\n');
    let (Some(from_header), Some(to_header)) = (all.next(), all.next()) else {
        return text.to_owned();
    };
    let body: Vec<&str> = all.collect();

    let mut placed: Vec<Placed> = Vec::with_capacity(body.len());
    let (mut first, mut second) = (0usize, 0usize);
    for line in &body {
        let kind = Kind::of(line);
        placed.push(Placed {
            kind,
            first: (kind != Kind::Added).then(|| {
                first += 1;
                first
            }),
            second: (kind != Kind::Removed).then(|| {
                second += 1;
                second
            }),
        });
    }

    let mut out = format!("{from_header}\n{to_header}");

    for (start, end) in hunks(&placed, context) {
        let within = &placed[start..=end];

        let side = |at: Side| {
            let count = within.iter().filter_map(at).count();
            // A side with no line in the hunk is written at the position it
            // had reached, which is what `git` does with the `-0,0` a file
            // the first version does not have gets.
            let at = within
                .iter()
                .find_map(at)
                .or_else(|| placed[..start].iter().rev().find_map(at))
                .unwrap_or(0);
            // One line is written as its start alone. The unified-diff rule,
            // and what a parser written against `git diff` expects.
            if count == 1 {
                format!("{at}")
            } else {
                format!("{at},{count}")
            }
        };

        out.push_str(&format!(
            "\n@@ -{} +{} @@",
            side(|placed| placed.first),
            side(|placed| placed.second),
        ));
        for line in &body[start..=end] {
            out.push('\n');
            out.push_str(line);
        }
    }

    out
}

/// The stretches of `placed` a trim keeps: every change, with `context` lines
/// either side, and two whose stretches meet joined into one.
///
/// Joining them where they touch as well as where they overlap is what makes
/// the rule `git`'s: two changes share a hunk when at most `2 * context`
/// unchanged lines lie between them, and a pair further apart than that is two
/// hunks with the lines between them dropped. Worth having because a reader
/// counting hunks in one of these answers is counting what it would count in
/// a patch from anywhere else.
fn hunks(placed: &[Placed], context: usize) -> Vec<(usize, usize)> {
    let mut hunks: Vec<(usize, usize)> = Vec::new();

    for (at, line) in placed.iter().enumerate() {
        if line.kind == Kind::Context {
            continue;
        }

        let start = at.saturating_sub(context);
        let end = (at + context).min(placed.len().saturating_sub(1));

        match hunks.last_mut() {
            Some((_, last)) if *last + 1 >= start => *last = (*last).max(end),
            _ => hunks.push((start, end)),
        }
    }

    hunks
}

impl Tool for GetFileDiff {
    const NAME: &'static str = "get_file_diff";
    const TITLE: &'static str = "Get file diff";
    const DESCRIPTION: &'static str = "\
        Read one file's diff out of a comparison you have already made, so \
        you can see what actually changed in it. Takes the handle \
        `diff_package_versions` gave you and a path from that comparison's \
        tree. Answers with a unified diff: a `--- from` / `+++ to` header, \
        `@@` hunks, and one line per change written as a sign, a space and \
        the line. Three things are worth knowing before you read one. \
        `isDiff` is false when the text is not a diff at all — a file both \
        versions ship byte for byte comes back as itself, and a path neither \
        version has comes back as a sentence saying so; read a false as a \
        file rather than parsing it as a patch. A file that was renamed needs \
        `old_path` as well, which the tree gives you beside it — without it \
        there is no file at that path in the first version and the answer is \
        the whole file, added. And `context_lines` is how many unchanged \
        lines come with each change, three by default: a short answer usually \
        means the rest of the file was left out rather than that little \
        changed, and `full` is how you ask for all of it. A long diff is cut \
        short, and the answer says so and gives the whole thing's size.";

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
        let comparison = diff_package_versions::compare(&args.handle, ctx).await?;

        comparison.file_patch(ctx, args.asked()).await
    }
}

/// What a caller asked for about one file, once the comparison it is part of
/// is settled.
///
/// [`Args`] without the handle, because by the time anything is rendered the
/// handle has been spent: the comparison it named carries it, and a second
/// copy here would be one that could name a different comparison. Public
/// because it is what
/// [`Comparison::file_patch`](super::diff_package_versions::Comparison::file_patch)
/// is asked, by this tool and by the `diffpack://diff/{handle}/file/{path}`
/// resource, which asks at the defaults a URI has room for.
pub struct OneFile<'a> {
    pub path: &'a str,
    pub old_path: Option<&'a str>,
    pub context_lines: ContextLines,
    pub max_bytes: Option<page::MaxBytes>,
}

impl Args {
    /// This call, without the handle it named a comparison with.
    pub fn asked(&self) -> OneFile<'_> {
        OneFile {
            path: &self.path,
            old_path: self.old_path.as_deref(),
            context_lines: self.context_lines,
            max_bytes: self.max_bytes,
        }
    }
}

/// One file's patch, out of a rendering that has already been made.
///
/// The trim and the cut, which is everything this tool does to the engine's
/// output and nothing it does to work out what the output is. Public for one
/// caller:
/// [`Comparison::file_patch`](crate::tools::diff_package_versions::Comparison::file_patch),
/// which answers one file's patch for this tool and for the resource that
/// reads it, and hands every patch it finds or renders through here. A
/// rendering arrives two ways — made just now out of both versions' contents,
/// or made by the cache when the entry was written — and those two are the
/// same bytes by [ADR
/// 0013](../../docs/adr/0013-the-patch-renderer-lives-in-the-engine-seam.md).
/// What is done to them afterwards has to be the same as well, or a
/// remembered patch is a differently trimmed one.
///
/// Here and not beside the lookup, because the rule it applies is this
/// tool's: `context_lines` is its argument, and [`trim`] is what that
/// argument means.
///
/// `context_lines` and `max_bytes` are the caller's either way. They are
/// presentation and not part of what was rendered, which is why a cached
/// patch can answer a call that asks for a different amount of context from
/// the one before it.
pub fn presented(rendered: &engine::Patch, asked: &OneFile<'_>) -> Patch {
    // Only a diff is trimmed. The other two answers are a file's own
    // content and a sentence, and neither has a header to keep or a
    // change to keep lines around.
    let text: Cow<'_, str> = match asked.context_lines {
        ContextLines::Around(lines) if rendered.is_diff => Cow::Owned(trim(&rendered.data, lines)),
        _ => Cow::Borrowed(&rendered.data),
    };

    Patch {
        excerpt: page::truncate(&text, asked.max_bytes),
        is_diff: rendered.is_diff,
    }
}
