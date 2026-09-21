//! The response ceiling: what fits under it, and how to say what did not.
//!
//! Vercel caps a function's response body at 4.5 MB. A response over it is
//! not a truncated answer — it is a platform error with nothing in it for the
//! client, so every tool that returns a list, a tree, a file or a patch has
//! to stay under it. This module is the one place that knows the number.

use schemars::JsonSchema;
use serde::Serialize;

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
#[derive(Debug, Clone, PartialEq, Eq, Serialize, JsonSchema)]
pub struct Page<T> {
    /// The items on this page, in the sequence's own order.
    pub items: Vec<T>,

    /// Where to resume. Absent when this page ends the sequence.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<String>,

    /// How many items the whole sequence has.
    pub total: usize,
}

/// Take as much of `items` as fits, starting where `cursor` says.
///
/// The caller supplies items. It does not count bytes, does not encode a
/// cursor, and does not decide what "too big" means.
pub fn paginate<T: Serialize>(
    items: impl IntoIterator<Item = T>,
    limit: Option<u32>,
    cursor: Option<&str>,
) -> Result<Page<T>, Failure> {
    let start = match cursor {
        Some(cursor) => Cursor::decode(cursor)?.offset,
        None => 0,
    };
    let wanted = limit.map_or(DEFAULT_LIMIT, |asked| (asked as usize).clamp(1, MAX_LIMIT));

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
    let next_cursor = (reached < total).then(|| Cursor::at(reached).encode());

    Ok(Page {
        items: taken,
        next_cursor,
        total,
    })
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

/// Where a walk resumes.
struct Cursor {
    offset: usize,
}

impl Cursor {
    fn at(offset: usize) -> Self {
        Self { offset }
    }

    fn encode(&self) -> String {
        format!("{CURSOR_VERSION}:{}", self.offset)
    }

    /// Read back a cursor this module wrote.
    ///
    /// A cursor is opaque to a client — the specification says so — which is
    /// exactly why a malformed one is the client's mistake rather than
    /// something a model can fix by rewording. It takes the protocol channel.
    fn decode(cursor: &str) -> Result<Self, Failure> {
        let refused = || Failure::InvalidParams {
            message: format!(
                "`{cursor}` is not a cursor. Pass back the `next_cursor` from the previous \
                 page unchanged, or omit it to start from the beginning."
            ),
        };

        let (version, offset) = cursor.split_once(':').ok_or_else(refused)?;
        if version != CURSOR_VERSION {
            return Err(refused());
        }

        Ok(Self::at(offset.parse().map_err(|_| refused())?))
    }
}

/// The cursor format's version, and the whole of what makes it one format.
const CURSOR_VERSION: &str = "p1";
