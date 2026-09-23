//! The response ceiling: what fits under it, and how to say what did not.
//!
//! Vercel caps a function's response body at 4.5 MB. A response over it is
//! not a truncated answer — it is a platform error with nothing in it for the
//! client, so every tool that returns a list, a tree, a file or a patch has
//! to stay under it. This module is the one place that knows the number.
//!
//! It also owns the arguments a caller narrows a sequence with before any of
//! it is paged: where to resume, how many to take, and which directory's
//! [`Subtree`] to walk. Each carries a rule an agent has to read, so each is a
//! type that writes its own schema, and the module that owns the rule is the
//! one that writes it.

use std::borrow::Cow;

use schemars::{json_schema, JsonSchema, Schema, SchemaGenerator};
use serde::{de, Deserialize, Deserializer, Serialize, Serializer};

use crate::error::Failure;

/// Vercel's cap on a function's response body.
///
/// A platform fact, not a preference. It is the whole body: the JSON-RPC
/// frame, the MCP result, and everything a tool put in it.
pub const RESPONSE_CEILING: usize = 4_500_000;

/// What the frame and the result wrapper are allowed to cost.
///
/// Generous on purpose. It is not measured, because measuring it would tie
/// this module to a particular envelope, and the thing being reserved
/// against is the one that grows when a protocol revision adds a field.
const ENVELOPE_RESERVE: usize = 64 * 1024;

/// How many serialised bytes of a tool's own answer fit under
/// [`RESPONSE_CEILING`].
///
/// Three times smaller than the ceiling, which is the part five separate
/// implementations of this would each have had to notice. A tool's answer
/// crosses the wire **twice**: `tools::invoke` hands it to
/// `CallToolResult::structured`, which sets `structuredContent` to the value
/// and *also* mirrors `value.to_string()` into a text block, because a client
/// that cannot read structured output still has to see the answer. The mirror
/// is a JSON string, so every `"` and `\` in it is escaped — at worst
/// doubling it.
///
/// So a payload of `n` serialised bytes costs `n` for the structured copy and
/// up to `2n` for the mirror. The ceiling is on the sum.
///
/// Worst case rather than typical, deliberately: real JSON escapes nearer
/// 15% than 100%, so this leaves most of a megabyte unused on an ordinary
/// answer. That is the right trade. Being wrong the other way does not
/// produce a slightly large response — it produces a platform error with
/// nothing in it that this repository can explain.
pub const PAYLOAD_CEILING: usize = (RESPONSE_CEILING - ENVELOPE_RESERVE) / 3;

/// How many items a page holds when the caller does not say.
///
/// Conservative, as #11 and #14 both ask: a page is what an agent reads
/// before deciding whether it wants more, and a thousand tree entries is not
/// that. The ceiling is the protection; this is the politeness.
pub const DEFAULT_LIMIT: usize = 200;

/// The most items a page will hold however large a `limit` is asked for.
///
/// A ceiling on the count as well as on the bytes, because the two protect
/// against different things: the bytes stop a response the platform refuses,
/// and this stops an answer nobody asked to read.
pub const MAX_LIMIT: usize = 1_000;

/// As much of a sequence as fits, and how to ask for the rest.
///
/// `total` is the length of the whole sequence and not of `items`. That is
/// the field a client needs to know there is more, and the one a tool that
/// counted its own answer would get wrong.
///
/// `next_cursor` travels as `nextCursor`, which is how the specification
/// spells the field on every other paginated result a client reads. A page
/// that named it otherwise would be this one server's spelling of the one
/// thing a client is meant to pass back without looking at it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Page<T> {
    /// The items on this page, in the sequence's own order.
    pub items: Vec<T>,

    /// Where to resume. Absent when this page ends the sequence.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<Cursor>,

    /// How many items the whole sequence has.
    pub total: usize,
}

/// Take as much of `items` as fits, starting where `cursor` says.
///
/// The caller supplies items. It does not count bytes, does not encode a
/// cursor, and does not decide what "too big" means.
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

        // Every item is counted, including the ones past the end of the page:
        // `total` is the sequence's length and a caller that stopped early
        // would be reporting the page's.
        if full || position < start || taken.len() >= wanted {
            continue;
        }

        // Plus one for the separator this item costs inside the array it
        // lands in. A thousand of those is a rounding error against the
        // envelope reserve, but counting them is cheaper than arguing about
        // whether it is.
        let cost = measure(&item)? + 1;

        if spent + cost > PAYLOAD_CEILING {
            // An empty page means this one item is over the ceiling by
            // itself, so there is no smaller `limit` and no later cursor that
            // would return it. Refusing is the only honest answer; the
            // refusal says where to resume so that it is still an answer.
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

/// How many items `limit` allows, filled in when it is absent.
///
/// The same number [`paginate`] would take, for the caller that has to ask a
/// source for that many *before* it has a sequence to paginate: a search is
/// answered by somebody else's server, and asking it for two hundred hits to
/// return ten would be spending their bandwidth to be polite with ours.
///
/// `u32` because what it is for is a number in a URL.
pub fn wanted(limit: Option<Limit>) -> u32 {
    let wanted = limit.map_or(DEFAULT_LIMIT, Limit::items);
    u32::try_from(wanted).unwrap_or(u32::MAX)
}

/// How many bytes `item` occupies once serialised.
///
/// The one definition of "too big" in this crate. It is the encoded length
/// and not a guess from a length or a field count, because the difference
/// between the two is a path that happened to be long — and a response that
/// is over the ceiling by a path is over the ceiling.
fn measure<T: Serialize>(item: &T) -> Result<usize, Failure> {
    // An item that will not serialise is a bug in the tool that built it, not
    // something the caller did, so it takes the internal channel — the same
    // call `tools::invoke` would have made a moment later.
    serde_json::to_vec(item)
        .map(|bytes| bytes.len())
        .map_err(|_| Failure::Internal {
            doing: "measuring an answer against the response ceiling",
        })
}

// ---------------------------------------------------------------------------
// The two arguments an agent sees
// ---------------------------------------------------------------------------
//
// `limit` and `cursor` are the whole of this module's surface on the wire, and
// they arrive as tool arguments. They are types rather than a `u32` and a
// `String` for the same reason `Registry` is a type and not a string: the
// schema a tool declares is where an agent reads the rule, so the module that
// owns the rule has to be the one that writes the schema. A tool spelling out
// `limit: Option<u32>` with a sentence about the default would be naming the
// number again, in the one copy no test compares against `MAX_LIMIT`.

/// How many items one page was asked for.
///
/// A number on the wire, clamped here rather than by the tool that was handed
/// it. Out of range is not a refusal: a client that asked for five thousand
/// entries still gets a page, and `total` is what tells it there are more.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(transparent)]
pub struct Limit(u32);

impl Limit {
    /// A limit a caller asked for, unclamped.
    ///
    /// `const` so that a tool's own documented default — if it ever wants one
    /// narrower than [`DEFAULT_LIMIT`] — is a constant rather than a call.
    pub const fn new(asked: u32) -> Self {
        Self(asked)
    }

    /// How many items this limit actually allows.
    ///
    /// The clamp is here, not at the boundary, so that a `Limit` built any
    /// other way than by deserialising one is held to the same range.
    fn items(self) -> usize {
        (self.0 as usize).clamp(1, MAX_LIMIT)
    }
}

/// The range and the default, where an agent reads them.
///
/// [`DEFAULT_LIMIT`] and [`MAX_LIMIT`] are written into the schema rather than
/// into each tool's prose, so a tool that takes a `Limit` documents the
/// numbers that bind and cannot document any others. Changing `MAX_LIMIT`
/// changes every tool's schema in the same commit.
impl JsonSchema for Limit {
    fn schema_name() -> Cow<'static, str> {
        "Limit".into()
    }

    fn schema_id() -> Cow<'static, str> {
        concat!(module_path!(), "::Limit").into()
    }

    /// Inline rather than a `$ref`. The reader is a model deciding what to
    /// pass, and a bound it has to resolve a reference to learn is a bound it
    /// will guess at instead.
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

/// The most of one blob a caller asked for.
///
/// The third of this module's wire types, and here for the reason ADR 0005
/// gives for the other two: the rule a caller has to know is *this* module's,
/// so this module writes the schema that carries it. A tool declaring
/// `max_bytes: Option<u32>` with a sentence of its own would be naming
/// [`PAYLOAD_CEILING`] again, in the copy no test compares against it — and
/// it would be naming it once per blob-shaped tool, which is #12 and #15
/// today.
///
/// Unlike [`Limit`] it has no default of its own to declare. Omitting it
/// means the ceiling, because the ceiling is what [`truncate`] falls back to,
/// and a "default" written into the schema would be a second answer to a
/// question that already has one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(transparent)]
pub struct MaxBytes(u64);

impl MaxBytes {
    /// A cap a caller asked for, unclamped.
    pub const fn new(asked: u64) -> Self {
        Self(asked)
    }

    /// How many bytes this cap actually allows.
    ///
    /// Only the floor is applied here. The ceiling is not, because
    /// [`truncate`] stops at [`PAYLOAD_CEILING`] whatever it was handed —
    /// clamping to it here as well would be a second copy of the one number
    /// this module exists to hold once.
    fn bytes(self) -> usize {
        usize::try_from(self.0).unwrap_or(usize::MAX).max(1)
    }
}

/// The ceiling, where an agent reads it.
///
/// `maximum` is [`PAYLOAD_CEILING`] because that is the most any answer can
/// carry, and a caller asking for more is narrowed rather than refused — the
/// same shape as [`Limit`]'s clamp, and for the same reason: a file that is
/// larger than a response is still a file worth reading the start of.
impl JsonSchema for MaxBytes {
    fn schema_name() -> Cow<'static, str> {
        "MaxBytes".into()
    }

    fn schema_id() -> Cow<'static, str> {
        concat!(module_path!(), "::MaxBytes").into()
    }

    /// Inline rather than a `$ref`, for [`Limit`]'s reason: a bound a model
    /// has to resolve a reference to learn is a bound it will guess at.
    fn inline_schema() -> bool {
        true
    }

    fn json_schema(_generator: &mut SchemaGenerator) -> Schema {
        json_schema!({
            "type": "integer",
            "minimum": 1,
            "maximum": PAYLOAD_CEILING,
            "description": format!(
                "The most of this text to return, in bytes. Omit it to get as much as \
                 fits. It can only ask for less: a value above {PAYLOAD_CEILING} is \
                 narrowed to that rather than refused, because a larger response is one \
                 the platform drops instead of shortening. Either way the answer says \
                 whether it was cut and how many bytes the whole thing is, so a short \
                 answer is never the whole story by omission.",
            ),
        })
    }
}

/// Where a walk resumes.
///
/// Opaque to a client: it is what the previous page handed back, passed in
/// unchanged. One format across every paginating tool, which is the whole of
/// what makes it a format rather than something two tools could spell
/// differently.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cursor {
    offset: usize,
}

impl Cursor {
    /// The cursor that resumes at `offset`.
    pub fn at(offset: usize) -> Self {
        Self { offset }
    }

    /// The cursor as it travels.
    pub fn encode(&self) -> String {
        format!("{CURSOR_VERSION}:{}", self.offset)
    }

    /// Read back a cursor this module wrote.
    ///
    /// A cursor is opaque to a client — the specification says so — which is
    /// exactly why a malformed one is the client's mistake rather than
    /// something a model can fix by rewording. It takes the protocol channel.
    ///
    /// Public for the caller that holds a cursor as a string rather than as a
    /// tool argument: the resource layer (#16) reads one out of a URI, where
    /// there is no schema to have refused it first.
    pub fn decode(cursor: &str) -> Result<Self, Failure> {
        parse(cursor).map_err(|message| Failure::InvalidParams { message })
    }
}

/// Decode `cursor`, or say what to pass instead.
///
/// The message names `nextCursor` rather than the format, because a client
/// that wrote its own cursor needs to be told to stop rather than told how to
/// write a better one.
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

/// On the wire a cursor is the one string [`Cursor::encode`] produces.
///
/// What a page returns and what the next call takes are the same thing,
/// without either end saying how a cursor is spelled.
impl Serialize for Cursor {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.encode())
    }
}

/// Reading an argument is decoding it.
///
/// This is what makes a paginating tool's `-32602` automatic: `tools::invoke`
/// deserialises a handler's `Args` before the handler runs, so a cursor that
/// is not ours never reaches one and no handler has to remember to check.
/// The same property [`crate::handle::DiffHandle`] has, for the same reason.
impl<'de> Deserialize<'de> for Cursor {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        // `Cow` rather than `&str`: a tool's arguments arrive as a parsed
        // `serde_json::Value`, which owns its strings and has nothing to
        // borrow from.
        let text = Cow::<str>::deserialize(deserializer)?;
        parse(&text).map_err(de::Error::custom)
    }
}

/// The `cursor` field every paginating tool declares.
///
/// A string with the rule where the field is: an agent told only that this is
/// a string will eventually build one out of an offset, and a cursor a client
/// wrote for itself is the one case this format refuses.
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
                beginning. It is opaque and it is not an index — a cursor written by \
                hand is refused.",
        })
    }
}

/// The cursor format's version, and the whole of what makes it one format.
const CURSOR_VERSION: &str = "p1";

// ---------------------------------------------------------------------------
// Which part of a sequence: one Subtree
// ---------------------------------------------------------------------------
//
// Not the ceiling, and here anyway. `cursor` says where in a sequence to
// resume and `limit` how much of it to take; this says which sequence, when
// the whole one is a directory tree and a caller wants one directory of it.
// Two tools take that argument, and each wrote the rule out for itself — the
// description, and the trimming of a slash — until the two copies disagreed
// about `/` (#97). The rule is a schema an agent reads and a
// normalisation a handler must not redo, which is `Limit`'s shape exactly, so
// it is a type that writes its own schema in the module that already owns
// those. A module of its own for one type is what ADR 0013 turned down.

/// A directory whose Subtree is asked for.
///
/// Normalised once, when it is built: a trailing slash a caller may or may not
/// have written is gone, so `src/` and `src` are one directory. What is left
/// of `/`, or of nothing, is nothing — and nothing is the root, so `/` and
/// `""` ask for everything, the same as omitting the argument. `/` is not a
/// refusal: it names a directory that exists (ADR 0017).
///
/// A directory, not a string to match on. `sr` does not narrow to `src/`, and
/// `lib` does not swallow `libs/`, because [`Subtree::contains`] compares at
/// the separator. And a directory is not inside its own Subtree: `src` is not
/// under `src`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Subtree {
    /// Empty for the root; otherwise a directory with no trailing slash.
    directory: String,
}

impl Subtree {
    /// The Subtree under `directory`, normalised.
    ///
    /// Public for the caller that builds a tool's arguments by hand rather
    /// than from JSON — a test asking about the answer's type — so that it is
    /// held to the same rule as one that arrived on the wire.
    pub fn new(directory: &str) -> Self {
        Self {
            directory: directory.trim_end_matches('/').to_owned(),
        }
    }

    /// Whether `path` is inside this Subtree.
    ///
    /// Everything is inside the root. Otherwise `path` has to continue past
    /// the directory with a separator, which is the whole of what keeps `sr`
    /// from matching `src/lib.rs` — and what keeps the directory out of its
    /// own Subtree, since `src` does not continue past `src` at all.
    pub fn contains(&self, path: &str) -> bool {
        self.directory.is_empty()
            || path
                .strip_prefix(&self.directory)
                .is_some_and(|rest| rest.starts_with('/'))
    }
}

/// Reading an argument is normalising it, so no handler trims a slash.
impl<'de> Deserialize<'de> for Subtree {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        // `Cow` for `Cursor`'s reason: the arguments arrive as a parsed
        // `serde_json::Value`, with nothing to borrow from.
        let text = Cow::<str>::deserialize(deserializer)?;
        Ok(Self::new(&text))
    }
}

/// The rule, where an agent reads it, written once for every tool that takes
/// a directory.
///
/// It says "all of it" rather than "the whole archive" or "the whole
/// comparison", because it is one sentence for both and each tool's own
/// description already says what it lists.
impl JsonSchema for Subtree {
    fn schema_name() -> Cow<'static, str> {
        "Subtree".into()
    }

    fn schema_id() -> Cow<'static, str> {
        concat!(module_path!(), "::Subtree").into()
    }

    /// Inline rather than a `$ref`, for [`Limit`]'s reason.
    fn inline_schema() -> bool {
        true
    }

    fn json_schema(_generator: &mut SchemaGenerator) -> Schema {
        json_schema!({
            "type": "string",
            "description": "\
                Only what is inside this directory, one level or many: `src`, or \
                `src/util`. A trailing slash is allowed and makes no difference. \
                It names a directory and is not matched by characters, so `sr` does \
                not narrow to `src/`, and the directory itself is not in its own \
                subtree. Omit it, or pass `/`, for all of it. A path with nothing \
                under it is an empty page rather than an error: a file, or a \
                directory that is not there.",
        })
    }
}

// ---------------------------------------------------------------------------
// The other half: one blob, cut loudly
// ---------------------------------------------------------------------------

/// As much of one thing as fits, and the truth about the rest.
///
/// The blob-shaped half of this module. #12 returns a file's content and #15
/// a file's diff; neither is a sequence, so neither has a next cursor. What
/// they share with a [`Page`] is the ceiling and the rule that a cut is
/// stated rather than hidden — a silently cut file is how an agent concludes
/// a function does not exist.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, JsonSchema)]
pub struct Excerpt {
    /// The text. When it was cut, the last line of it says so and says how
    /// much you were given.
    pub text: String,

    /// False when `text` is the whole thing, true when it was cut and there
    /// is more you have not been shown.
    pub truncated: bool,

    /// How many bytes the whole thing is, whether or not it was cut.
    // The real total rather than the returned length. A tool reporting the
    // latter would be telling an agent that a file it has seen a tenth of is
    // a tenth long — and these three descriptions reach that agent, since a
    // tool flattens this into its own output schema.
    pub bytes: usize,
}

/// Return as much of `text` as fits, and say so when that is not all of it.
///
/// `max_bytes` is the caller's own cap, in the text's own bytes, for a tool
/// that wants less than everything. It narrows the cut; it cannot widen it,
/// because the ceiling is not the caller's to raise. Omitting it means the
/// ceiling alone — which is what makes "the server applies a default
/// regardless" (#12) true without a tool having to remember to ask.
pub fn truncate(text: &str, max_bytes: Option<MaxBytes>) -> Excerpt {
    let bytes = text.len();
    let raw_cap = max_bytes.map_or(usize::MAX, MaxBytes::bytes);

    // The marker is part of what has to fit, and its own length depends on
    // where the cut lands, so the room for content is the ceiling less the
    // longest the marker could ever be rather than less what it turns out to
    // be. Two more for the quotes around the JSON string.
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

    // `kept` is a sum of whole characters' widths, so it is a character
    // boundary and this slice cannot panic.
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

/// How many bytes `text` costs once escaped into the field that carries it.
///
/// The measurement [`truncate`] makes while it cuts, for the caller that only
/// needs the answer. Private because [`fits`] is that caller and there is no
/// other: a `pub` with nothing on the other side of it is surface this crate
/// would have to keep working.
fn encoded_len(text: &str) -> usize {
    text.chars().map(encoded_cost).sum()
}

/// Whether `text` fits whole in one answer.
///
/// The third shape this module answers for, beside a [`Page`] and an
/// [`Excerpt`], and the one with no smaller version of itself. A sequence too
/// long for one answer is paged and a blob too long is cut, because half a
/// file is still a readable half. A comparison's tree is neither: the first
/// nine tenths of one reads exactly like a whole one, and an agent looking
/// for a file in the last tenth is told it is not there. So what does not fit
/// is *replaced* by a statement about itself — [`crate::resources::diff`] is
/// the caller, and the statement is its to write.
///
/// [`PAYLOAD_CEILING`] is the bound, which is conservative here rather than
/// exact: that number is a third of the platform's because a tool's answer
/// crosses the wire twice, and a resource read carries its document once. The
/// margin is left where it is deliberately. Being wrong the other way is a
/// platform error with nothing in it, and a tree over a megabyte is one an
/// agent should be paging through whatever the ceiling allows.
pub fn fits(text: &str) -> bool {
    encoded_len(text) <= PAYLOAD_CEILING
}

/// The most the marker can cost, once escaped.
///
/// Two usize decimals is forty digits and the rest is a fixed ASCII phrase
/// with one newline in it, so eighty-odd is the real number and this is the
/// round one above it. Reserving the maximum rather than measuring the actual
/// marker is what breaks the circle: the marker names the cut, and the cut
/// has to leave room for the marker.
const MARKER_RESERVE: usize = 128;

/// What one character costs inside a JSON string.
///
/// `serde_json`'s escaping rules, which is the encoder every answer here goes
/// through. Non-ASCII is emitted as itself rather than as `\u` escapes, so a
/// multi-byte character costs its UTF-8 width and no more.
fn encoded_cost(character: char) -> usize {
    match character {
        '"' | '\\' | '\n' | '\r' | '\t' | '\u{08}' | '\u{0c}' => 2,
        // Every other control character is written out as `\u00XX`.
        control if (control as u32) < 0x20 => 6,
        other => other.len_utf8(),
    }
}
