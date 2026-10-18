use std::borrow::Cow;

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine as _;
use schemars::{json_schema, JsonSchema, Schema, SchemaGenerator};
use serde::{de, Deserialize, Deserializer, Serialize, Serializer};

use crate::cache_key::{DiffKey, SCHEMA};
use crate::engine;
use crate::error::Failure;
use crate::registry::Registry;

const HANDLE_VERSION: &str = "d1";

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Inputs {
    pub registry: Registry,
    pub package: String,
    pub from_version: String,
    pub to_version: String,
    pub similarity_threshold: f64,
    pub ignore_whitespace: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct DiffHandle {
    inputs: Inputs,
}

impl DiffHandle {
    pub fn mint(inputs: Inputs) -> Self {
        Self { inputs }
    }

    pub fn inputs(&self) -> &Inputs {
        &self.inputs
    }

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

    pub fn diff_id(&self) -> String {
        self.key().diff_id()
    }

    pub fn encode(&self) -> String {
        let payload = serde_json::to_vec(&Payload::of(self)).unwrap_or_default();
        format!("{HANDLE_VERSION}:{}", URL_SAFE_NO_PAD.encode(payload))
    }

    pub fn decode(text: &str) -> Result<Self, Failure> {
        parse(text).map_err(|message| Failure::InvalidParams { message })
    }
}

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

fn malformed(text: &str) -> String {
    format!(
        "`{text}` is not a diff handle. A handle comes from `diff_package_versions` and is \
         passed back unchanged."
    )
}

impl Serialize for DiffHandle {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.encode())
    }
}

impl<'de> Deserialize<'de> for DiffHandle {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = Cow::<str>::deserialize(deserializer)?;
        parse(&text).map_err(de::Error::custom)
    }
}

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
                also fixes how the comparison was made, `similarity_threshold` and \
                `ignore_whitespace` included, so neither is accepted beside it: to compare \
                with different ones, call `diff_package_versions` again for a new handle. \
                It is not a `diff_id` and it is not written by hand: a handle whose \
                contents do not agree with each other is refused.",
        })
    }
}
