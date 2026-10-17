use std::borrow::Cow;

use schemars::{json_schema, JsonSchema, Schema, SchemaGenerator};
use serde::{de, Deserialize, Deserializer, Serialize};

use crate::engine;
use crate::error::Failure;
use crate::handle::DiffHandle;
use crate::page::{self, Excerpt};
use crate::tools::{diff_package_versions, Call, Tool};

pub struct GetFileDiff;

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[schemars(
    description = "What a caller asks for.\n\
    \n\
    The doc comments below are read by a model — see [`super`]."
)]
#[serde(deny_unknown_fields)]
pub struct Args {
    pub handle: DiffHandle,

    #[schemars(
        description = "Where the file is in the second version, with the archive's top-level\n\
        directory already removed: `src/index.js`, never\n\
        `zod-4.0.0/src/index.js`. For a file the second version does not have,\n\
        where it was in the first."
    )]
    pub path: String,

    #[schemars(
        description = "Where the file was in the first version, when it moved. Pass the\n\
        `old_path` the comparison gave you for a renamed file; omit it for\n\
        everything else."
    )]
    #[serde(default)]
    pub old_path: Option<String>,

    #[serde(default)]
    pub context_lines: ContextLines,

    #[serde(default)]
    pub max_bytes: Option<page::MaxBytes>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, schemars::JsonSchema)]
#[schemars(description = "One file's patch, as much of it as fits.")]
#[serde(rename_all = "camelCase")]
pub struct Patch {
    #[serde(flatten)]
    pub excerpt: Excerpt,

    #[schemars(
        description = "True when `text` is a diff — a `--- from` / `+++ to` header and one\n\
        line per change. False when it is not, and then it is the file's own\n\
        content: a file both versions have byte for byte has no diff to show,\n\
        and a path neither version has is a sentence saying so. Render a\n\
        `false` as a file rather than trying to parse it as a patch."
    )]
    pub is_diff: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContextLines {
    Around(u32),

    Full,
}

pub const DEFAULT_CONTEXT: u32 = 3;

impl Default for ContextLines {
    fn default() -> Self {
        Self::Around(DEFAULT_CONTEXT)
    }
}

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

#[derive(Deserialize)]
#[serde(untagged)]
enum Asked {
    Around(u32),
    Word(String),
}

impl JsonSchema for ContextLines {
    fn schema_name() -> Cow<'static, str> {
        "ContextLines".into()
    }

    fn schema_id() -> Cow<'static, str> {
        concat!(module_path!(), "::ContextLines").into()
    }

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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Context,
    Removed,
    Added,
}

impl Kind {
    fn of(line: &str) -> Self {
        match line.as_bytes().first() {
            Some(b'-') => Self::Removed,
            Some(b'+') => Self::Added,
            _ => Self::Context,
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct Placed {
    kind: Kind,
    first: Option<usize>,
    second: Option<usize>,
}

type Side = fn(&Placed) -> Option<usize>;

fn trim(text: &str, context: u32) -> String {
    let context = context as usize;

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
            let at = within
                .iter()
                .find_map(at)
                .or_else(|| placed[..start].iter().rev().find_map(at))
                .unwrap_or(0);
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

    const READ_ONLY: bool = true;

    const IDEMPOTENT: bool = true;

    const OPEN_WORLD: bool = true;

    type Args = Args;
    type Output = Patch;

    async fn call(args: Args, call: &Call) -> Result<Patch, Failure> {
        let comparison = diff_package_versions::compare(&args.handle, call).await?;

        comparison.file_patch(call, args.asked()).await
    }
}

pub struct OneFile<'a> {
    pub path: &'a str,
    pub old_path: Option<&'a str>,
    pub context_lines: ContextLines,
    pub max_bytes: Option<page::MaxBytes>,
}

impl Args {
    pub fn asked(&self) -> OneFile<'_> {
        OneFile {
            path: &self.path,
            old_path: self.old_path.as_deref(),
            context_lines: self.context_lines,
            max_bytes: self.max_bytes,
        }
    }
}

pub fn presented(rendered: &engine::Patch, asked: &OneFile<'_>) -> Patch {
    let text: Cow<'_, str> = match asked.context_lines {
        ContextLines::Around(lines) if rendered.is_diff => Cow::Owned(trim(&rendered.data, lines)),
        _ => Cow::Borrowed(&rendered.data),
    };

    Patch {
        excerpt: page::truncate(&text, asked.max_bytes),
        is_diff: rendered.is_diff,
    }
}
