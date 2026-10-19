use std::borrow::Cow;

use schemars::{json_schema, JsonSchema, Schema, SchemaGenerator};
use serde::{de, Deserialize, Deserializer, Serialize, Serializer};

use crate::error::Failure;

pub const RESPONSE_CEILING: usize = 4_500_000;

const ENVELOPE_RESERVE: usize = 64 * 1024;

pub const PAYLOAD_CEILING: usize = (RESPONSE_CEILING - ENVELOPE_RESERVE) / 3;

pub const DEFAULT_LIMIT: usize = 200;

pub const MAX_LIMIT: usize = 1_000;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, JsonSchema)]
#[schemars(
    description = "As much of a sequence as fits, and how to ask for the rest.\n\
    \n\
    `total` is the length of the whole sequence and not of `items`. That is\n\
    the field a client needs to know there is more, and the one a tool that\n\
    counted its own answer would get wrong.\n\
    \n\
    `next_cursor` travels as `nextCursor`, which is how the specification\n\
    spells the field on every other paginated result a client reads. A page\n\
    that named it otherwise would be this one server's spelling of the one\n\
    thing a client is meant to pass back without looking at it."
)]
#[serde(rename_all = "camelCase")]
pub struct Page<T> {
    #[schemars(description = "The items on this page, in the sequence's own order.")]
    pub items: Vec<T>,

    #[schemars(description = "Where to resume. Absent when this page ends the sequence.")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<Cursor>,

    #[schemars(description = "How many items the whole sequence has.")]
    pub total: usize,
}

pub fn paginate<T: Serialize>(
    items: impl IntoIterator<Item = T>,
    limit: Option<Limit>,
    cursor: Option<Cursor>,
) -> Result<Page<T>, Failure> {
    let start = cursor.map_or(0, |cursor| cursor.offset);
    let wanted = limit.map_or(DEFAULT_LIMIT, Limit::items);

    let mut taken = Vec::new();
    let mut spent = 0;
    let mut total = 0;
    let mut full = false;

    for item in items {
        let position = total;
        total += 1;

        if full || position < start || taken.len() >= wanted {
            continue;
        }

        let cost = measure(&item)? + 1;

        if spent + cost > PAYLOAD_CEILING {
            if taken.is_empty() {
                return Err(Failure::ItemTooLarge {
                    position,
                    bytes: cost - 1,
                    ceiling: PAYLOAD_CEILING,
                    resume: Cursor::at(position + 1).encode(),
                });
            }
            full = true;
            continue;
        }

        spent += cost;
        taken.push(item);
    }

    let reached = start + taken.len();
    let next_cursor = (reached < total).then(|| Cursor::at(reached));

    Ok(Page {
        items: taken,
        next_cursor,
        total,
    })
}

pub fn wanted(limit: Option<Limit>) -> u32 {
    let wanted = limit.map_or(DEFAULT_LIMIT, Limit::items);
    u32::try_from(wanted).unwrap_or(u32::MAX)
}

fn measure<T: Serialize>(item: &T) -> Result<usize, Failure> {
    serde_json::to_vec(item)
        .map(|bytes| bytes.len())
        .map_err(|_| Failure::Internal {
            doing: "measuring an answer against the response ceiling",
        })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(transparent)]
pub struct Limit(u32);

impl Limit {
    pub const fn new(asked: u32) -> Self {
        Self(asked)
    }

    fn items(self) -> usize {
        (self.0 as usize).clamp(1, MAX_LIMIT)
    }
}

impl JsonSchema for Limit {
    fn schema_name() -> Cow<'static, str> {
        "Limit".into()
    }

    fn schema_id() -> Cow<'static, str> {
        concat!(module_path!(), "::Limit").into()
    }

    fn inline_schema() -> bool {
        true
    }

    fn json_schema(_generator: &mut SchemaGenerator) -> Schema {
        json_schema!({
            "type": "integer",
            "minimum": 1,
            "maximum": MAX_LIMIT,
            "default": DEFAULT_LIMIT,
            "description": format!(
                "How many items to return on this page. Omit it for {DEFAULT_LIMIT}. \
                 A value outside 1 to {MAX_LIMIT} is clamped into that range rather than \
                 refused, and the answer's `total` is the whole sequence's length either \
                 way, so a page that came back short is not the end of it.",
            ),
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(transparent)]
pub struct MaxBytes(u64);

impl MaxBytes {
    pub const fn new(asked: u64) -> Self {
        Self(asked)
    }

    fn bytes(self) -> usize {
        usize::try_from(self.0).unwrap_or(usize::MAX).max(1)
    }
}

impl JsonSchema for MaxBytes {
    fn schema_name() -> Cow<'static, str> {
        "MaxBytes".into()
    }

    fn schema_id() -> Cow<'static, str> {
        concat!(module_path!(), "::MaxBytes").into()
    }

    fn inline_schema() -> bool {
        true
    }

    fn json_schema(_generator: &mut SchemaGenerator) -> Schema {
        json_schema!({
            "type": "integer",
            "minimum": 1,
            "maximum": PAYLOAD_CEILING,
            "description": format!(
                "The most of this text to return, in bytes, from 1 to {PAYLOAD_CEILING}. \
                 Omit it to get as much as fits. It can only ask for less: a value above \
                 {PAYLOAD_CEILING} is \
                 narrowed to that rather than refused, because a larger response is one \
                 the platform drops instead of shortening. Either way the answer says \
                 whether it was cut and how many bytes the whole thing is, so a short \
                 answer is never the whole story by omission.",
            ),
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cursor {
    offset: usize,
}

impl Cursor {
    pub fn at(offset: usize) -> Self {
        Self { offset }
    }

    pub fn encode(&self) -> String {
        format!("{CURSOR_VERSION}:{}", self.offset)
    }

    pub fn decode(cursor: &str) -> Result<Self, Failure> {
        parse(cursor).map_err(|message| Failure::InvalidParams { message })
    }
}

fn parse(cursor: &str) -> Result<Cursor, String> {
    let refused = || {
        format!(
            "`{cursor}` is not a cursor. Pass back the `nextCursor` from the previous \
             page unchanged, or omit it to start from the beginning."
        )
    };

    let (version, offset) = cursor.split_once(':').ok_or_else(refused)?;
    if version != CURSOR_VERSION {
        return Err(refused());
    }

    Ok(Cursor::at(offset.parse().map_err(|_| refused())?))
}

impl Serialize for Cursor {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.encode())
    }
}

impl<'de> Deserialize<'de> for Cursor {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = Cow::<str>::deserialize(deserializer)?;
        parse(&text).map_err(de::Error::custom)
    }
}

impl JsonSchema for Cursor {
    fn schema_name() -> Cow<'static, str> {
        "Cursor".into()
    }

    fn schema_id() -> Cow<'static, str> {
        concat!(module_path!(), "::Cursor").into()
    }

    fn inline_schema() -> bool {
        true
    }

    fn json_schema(_generator: &mut SchemaGenerator) -> Schema {
        json_schema!({
            "type": "string",
            "pattern": format!("^{CURSOR_VERSION}:[0-9]+$"),
            "description": "\
                Where to resume a walk of this sequence: the `nextCursor` from the \
                previous page, passed back unchanged. Omit it to start from the \
                beginning. Treat it as opaque rather than writing or editing one: \
                what it holds is this server's to change, and a value that is not in \
                the shape this server writes is refused.",
        })
    }
}

const CURSOR_VERSION: &str = "p1";

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Subtree {
    directory: String,
}

impl Subtree {
    pub fn directory(&self) -> Option<&str> {
        (!self.directory.is_empty()).then_some(self.directory.as_str())
    }

    pub fn contains(&self, path: &str) -> bool {
        self.directory.is_empty()
            || path
                .strip_prefix(&self.directory)
                .is_some_and(|rest| rest.starts_with('/'))
    }
}

impl<'de> Deserialize<'de> for Subtree {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = Cow::<str>::deserialize(deserializer)?;
        Ok(Self {
            directory: text.trim_end_matches('/').to_owned(),
        })
    }
}

impl JsonSchema for Subtree {
    fn schema_name() -> Cow<'static, str> {
        "Subtree".into()
    }

    fn schema_id() -> Cow<'static, str> {
        concat!(module_path!(), "::Subtree").into()
    }

    fn inline_schema() -> bool {
        true
    }

    fn json_schema(_generator: &mut SchemaGenerator) -> Schema {
        json_schema!({
            "type": "string",
            "description": "\
                Only what is inside this directory, one level or many: `src`, or \
                `src/util`, with the archive's top-level directory already removed, \
                so never `zod-4.0.0/src`. A trailing slash is allowed and makes no \
                difference. It names a directory and is not matched by characters, \
                so `sr` does not narrow to `src/`, and the directory itself is not in its own \
                subtree. Omit it, or pass `/` or an empty string, for all of it. \
                A path with nothing under it is an empty page rather than an \
                error: a file, or a directory that is not there.",
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, JsonSchema)]
#[schemars(
    description = "As much of one thing as fits, and the truth about the rest.\n\
    \n\
    The blob-shaped half of this module. #12 returns a file's content and #15\n\
    a file's diff; neither is a sequence, so neither has a next cursor. What\n\
    they share with a [`Page`] is the ceiling and the rule that a cut is\n\
    stated rather than hidden — a silently cut file is how an agent concludes\n\
    a function does not exist."
)]
pub struct Excerpt {
    #[schemars(
        description = "The text. When it was cut, the last line of it says so and says how\n\
        much you were given."
    )]
    pub text: String,

    #[schemars(
        description = "False when `text` is the whole thing, true when it was cut and there\n\
        is more you have not been shown."
    )]
    pub truncated: bool,

    #[schemars(description = "How many bytes the whole thing is, whether or not it was cut.")]
    pub bytes: usize,
}

pub fn truncate(text: &str, max_bytes: Option<MaxBytes>) -> Excerpt {
    let bytes = text.len();
    let raw_cap = max_bytes.map_or(usize::MAX, MaxBytes::bytes);

    let room = PAYLOAD_CEILING.saturating_sub(MARKER_RESERVE + 2);

    let mut kept = 0;
    let mut encoded = 0;
    for character in text.chars() {
        let raw = character.len_utf8();
        let cost = encoded_cost(character);
        if kept + raw > raw_cap || encoded + cost > room {
            break;
        }
        kept += raw;
        encoded += cost;
    }

    if kept == bytes {
        return Excerpt {
            text: text.to_owned(),
            truncated: false,
            bytes,
        };
    }

    let mut shown = text[..kept].to_owned();
    shown.push_str(&format!(
        "\n[truncated by diffpack: {kept} of {bytes} bytes shown]"
    ));

    Excerpt {
        text: shown,
        truncated: true,
        bytes,
    }
}

fn encoded_len(text: &str) -> usize {
    text.chars().map(encoded_cost).sum()
}

pub fn fits(text: &str) -> bool {
    encoded_len(text) <= PAYLOAD_CEILING
}

const MARKER_RESERVE: usize = 128;

fn encoded_cost(character: char) -> usize {
    match character {
        '"' | '\\' | '\n' | '\r' | '\t' | '\u{08}' | '\u{0c}' => 2,
        control if (control as u32) < 0x20 => 6,
        other => other.len_utf8(),
    }
}
