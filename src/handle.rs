//! The handle that passes between the tool that computes a diff and the ones
//! that read it back.
//!
//! `diff_package_versions` (#13) mints one; `get_diff_tree` (#14) and
//! `get_file_diff` (#15) take one, and the diff resources (#16) will. It
//! carries the `diff_id` — the cache lookup, and the string #27 needs — and
//! beside it the inputs that `diff_id` was minted from, so that a reading
//! tool whose entry has been evicted recomputes instead of refusing. See [ADR
//! 0006](../docs/adr/0006-the-handle-carries-its-inputs.md).

use std::borrow::Cow;

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine as _;
use schemars::{json_schema, JsonSchema, Schema, SchemaGenerator};
use serde::{de, Deserialize, Deserializer, Serialize, Serializer};

use crate::cache_key::{DiffKey, SCHEMA};
use crate::engine;
use crate::error::Failure;
use crate::registry::Registry;

/// The version prefix every handle carries.
const HANDLE_VERSION: &str = "d1";

/// What a caller asked for: the half of a [`DiffKey`] that comes from a tool's
/// arguments rather than from this build.
///
/// `Serialize` for one reader: `diffpack://diff/{handle}` (#16) answers with
/// what was compared beside the comparison, because a URI carries an opaque
/// handle and a document read out of a client's resource browser has no call
/// beside it saying what was asked for. It is derived rather than written out
/// there, so the field names an agent reads are these ones and there is no
/// second list to keep in step. Distinct from [`DiffHandle`]'s own
/// `Serialize`, which is the opaque string: that is how a handle *travels*
/// and this is what it says.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Inputs {
    /// The registry that publishes the package.
    pub registry: Registry,
    /// The package name, verbatim.
    pub package: String,
    /// The base version.
    pub from_version: String,
    /// The target version. Ordered: A→B is not B→A.
    pub to_version: String,
    /// Rename-detection threshold.
    pub similarity_threshold: f64,
    /// Whether whitespace was disregarded.
    pub ignore_whitespace: bool,
}

/// One diff, named and reproducible.
#[derive(Debug, Clone, PartialEq)]
pub struct DiffHandle {
    inputs: Inputs,
}

impl DiffHandle {
    /// Mint a handle for the diff `inputs` describe.
    ///
    /// The caller supplies its half and no more. The engine version and the
    /// schema are this build's, taken here rather than accepted, which is
    /// what keeps a handle from naming a diff computed by code that is not
    /// running.
    pub fn mint(inputs: Inputs) -> Self {
        Self { inputs }
    }

    /// What the caller asked for, for a tool that has to recompute.
    ///
    /// This is the whole of what a cache miss costs: with these a reading
    /// tool fetches both archives and diffs them again, and answers.
    pub fn inputs(&self) -> &Inputs {
        &self.inputs
    }

    /// The key this diff is cached under.
    pub fn key(&self) -> DiffKey {
        DiffKey {
            schema: SCHEMA,
            engine: engine::VERSION.to_owned(),
            registry: self.inputs.registry.id().to_owned(),
            package: self.inputs.package.clone(),
            from: self.inputs.from_version.clone(),
            to: self.inputs.to_version.clone(),
            similarity_threshold: self.inputs.similarity_threshold,
            ignore_whitespace: self.inputs.ignore_whitespace,
        }
    }

    /// The name of the diff this handle points at.
    pub fn diff_id(&self) -> String {
        self.key().diff_id()
    }

    /// The handle as it travels: a version prefix and a base64url payload.
    ///
    /// A payload that will not serialise produces an empty one, which is a
    /// handle that will not decode — the fail-closed direction, and cheaper
    /// than a panic inside a request. There is no such payload: every field
    /// is a string, a bool or a number, and a non-finite threshold cannot
    /// arrive from JSON in the first place.
    pub fn encode(&self) -> String {
        let payload = serde_json::to_vec(&Payload::of(self)).unwrap_or_default();
        format!("{HANDLE_VERSION}:{}", URL_SAFE_NO_PAD.encode(payload))
    }

    /// Read back a handle this server minted, verification included.
    ///
    /// A [`Failure::InvalidParams`], which is `-32602`: every way this can
    /// fail is something a client did, and none of them is anything a model
    /// can act on.
    pub fn decode(text: &str) -> Result<Self, Failure> {
        parse(text).map_err(|message| Failure::InvalidParams { message })
    }
}

/// What the encoded handle holds: the `diff_id`, and the whole key it was
/// computed from.
///
/// The fields are written out rather than flattened from a [`DiffKey`], and
/// they are in ASCII sort order, which is the order `docs/cache-key.md` puts
/// the canonical string's keys in. Both are on purpose: the payload is this
/// module's format and not the cache key's, so a field reordered in
/// `cache_key.rs` — a file held to a document — cannot silently change what a
/// handle looks like on the wire.
///
/// `deny_unknown_fields` because a handle is minted, never written: a payload
/// with a field this build does not know is not a handle from a future build
/// to be read leniently, it is something a client made up.
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Payload {
    diff_id: String,
    engine: String,
    from: String,
    ignore_whitespace: bool,
    package: String,
    registry: String,
    schema: u32,
    similarity_threshold: f64,
    to: String,
}

impl Payload {
    fn of(handle: &DiffHandle) -> Self {
        let key = handle.key();
        Self {
            diff_id: key.diff_id(),
            engine: key.engine,
            from: key.from,
            ignore_whitespace: key.ignore_whitespace,
            package: key.package,
            registry: key.registry,
            schema: key.schema,
            similarity_threshold: key.similarity_threshold,
            to: key.to,
        }
    }
}

/// Decode `text`, or say why it is not a handle this server will act on.
///
/// Four refusals, in the order that makes each one the honest answer. The
/// shape first, because nothing else can be checked until it parses. Then the
/// build, because a handle from another deployment fails the `diff_id` check
/// too, and "this does not match its inputs" would be a lie about what
/// happened. Then the registry, and last the `diff_id` itself.
fn parse(text: &str) -> Result<DiffHandle, String> {
    let encoded = text
        .strip_prefix(&format!("{HANDLE_VERSION}:"))
        .ok_or_else(|| malformed(text))?;
    let payload = URL_SAFE_NO_PAD
        .decode(encoded)
        .map_err(|_| malformed(text))?;
    let payload: Payload = serde_json::from_slice(&payload).map_err(|_| malformed(text))?;

    if payload.schema != SCHEMA || payload.engine != engine::VERSION {
        return Err(format!(
            "This handle was minted by a different build of this server — engine {}, \
             schema {} — and the diff it names was computed by code this one no longer \
             runs. Call `diff_package_versions` again for a handle this build can read.",
            payload.engine, payload.schema,
        ));
    }

    let registry = Registry::from_id(&payload.registry).ok_or_else(|| malformed(text))?;

    let handle = DiffHandle::mint(Inputs {
        registry,
        package: payload.package,
        from_version: payload.from,
        to_version: payload.to,
        similarity_threshold: payload.similarity_threshold,
        ignore_whitespace: payload.ignore_whitespace,
    });

    // The two halves against each other. Without this a handle whose inputs
    // were edited would name one diff and describe another, and every tool
    // that took it would answer confidently about the wrong package.
    if handle.diff_id() != payload.diff_id {
        return Err(
            "This handle's `diff_id` does not name the inputs beside it, so one of the two \
             was edited. A handle is minted by `diff_package_versions` and passed back \
             unchanged; it cannot be built or corrected by hand."
                .to_owned(),
        );
    }

    Ok(handle)
}

/// What a client is told when a handle is not one.
///
/// It does not say which of the four ways it was wrong. A client that got the
/// prefix, the base64 and the JSON right and the registry wrong was building
/// a handle rather than passing one back, and the answer to all of it is the
/// same call.
fn malformed(text: &str) -> String {
    format!(
        "`{text}` is not a diff handle. A handle comes from `diff_package_versions` and is \
         passed back unchanged."
    )
}

/// On the wire a handle is the one string [`DiffHandle::encode`] produces.
///
/// What a tool returns and what a tool takes, so that #13's answer and #14's
/// argument are the same thing without either of them saying how a handle is
/// spelled.
impl Serialize for DiffHandle {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.encode())
    }
}

/// Reading an argument is decoding it, verification included.
///
/// This is what makes a tool's `-32602` automatic: `tools::invoke`
/// deserialises a handler's `Args` before the handler runs and puts a
/// rejected argument on the protocol channel, so a handle that does not
/// decode never reaches a handler and no handler has to remember to check
/// one.
impl<'de> Deserialize<'de> for DiffHandle {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        // `Cow` rather than `&str`: a tool's arguments arrive as a parsed
        // `serde_json::Value`, which owns its strings and has nothing to
        // borrow from.
        let text = Cow::<str>::deserialize(deserializer)?;
        parse(&text).map_err(de::Error::custom)
    }
}

/// The `handle` field every diff-reading tool declares.
///
/// A string with a description where the field is, rather than a `$ref` a
/// model has to resolve: the reader is deciding what to pass, and #23 asks
/// that it can do that without documentation. The description says where a
/// handle comes from and that it is not written by hand, because the shape
/// alone would invite both.
impl JsonSchema for DiffHandle {
    fn schema_name() -> Cow<'static, str> {
        "DiffHandle".into()
    }

    fn schema_id() -> Cow<'static, str> {
        concat!(module_path!(), "::DiffHandle").into()
    }

    fn inline_schema() -> bool {
        true
    }

    fn json_schema(_generator: &mut SchemaGenerator) -> Schema {
        json_schema!({
            "type": "string",
            "pattern": format!("^{HANDLE_VERSION}:[A-Za-z0-9_-]+$"),
            "description": "\
                A handle for one diff, minted by `diff_package_versions` and passed back \
                exactly as it arrived. It carries the diff's inputs as well as its \
                `diff_id`, which is what lets this server recompute the diff when its \
                cached result has been evicted rather than asking you to start again. It \
                is not a `diff_id` and it is not written by hand: a handle whose contents \
                do not agree with each other is refused.",
        })
    }
}
