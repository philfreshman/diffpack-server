//! One module per tool.
//!
//! Every MCP tool this server offers is one file under here, holding its
//! definition and its handler together. [`Tool`] is the interface each one
//! implements, and the [`tools!`] list below is the only other place a tool's
//! name appears: adding one is a new file and a line.
//!
//! # Why the definition is derived rather than written
//!
//! The shape this replaces is a list of `Tool` definitions in one place and a
//! `match` on the name in another. The two drift, and the drift does not
//! fail to compile — it reaches a model as a tool whose declared schema
//! disagrees with what the handler reads, which is a confident wrong answer.
//!
//! So a tool writes down types, not JSON. [`Tool::Args`] is what the handler
//! is given and [`Tool::Output`] is what it returns; the input schema, the
//! output schema and the structured answer are all generated from those two
//! types, and a handler that reads a field its schema never declared is a
//! compile error rather than a runtime surprise. The rest of what #23 asks
//! for — a description, the read-only and idempotency hints — are required
//! associated items, so a tool that omits one does not compile either.
//!
//! That is the strictness on purpose: these are per-tool facts, and a default
//! would be a guess. `readOnlyHint` defaulting to false on a tool that only
//! reads costs an agent a confirmation prompt it did not need; defaulting to
//! true on one that writes is worse. A tool has to say.
//!
//! # What a tool may reach
//!
//! [`Ctx`], and nothing else it has to build itself.
//! `scripts/check-tool-seams.sh` enforces the rest: a module here cannot name
//! an HTTP client or the blob store. See
//! [`docs/architecture.md`](../../docs/architecture.md) and ADR 0002.

use std::future::Future;
use std::sync::Arc;

use rmcp::model::{CallToolResult, JsonObject, Tool as Definition, ToolAnnotations};
use rmcp::ErrorData;
use schemars::JsonSchema;
use serde::de::DeserializeOwned;
use serde::Serialize;

use crate::error::Failure;

/// Declare the tools, and build the collection and the dispatch from one list.
///
/// `$module::$type` both names the file and the implementation inside it, so
/// a module cannot be registered under another module's name and a module
/// that is never registered is never compiled in.
macro_rules! tools {
    ($($module:ident::$tool:ident),+ $(,)?) => {
        $(mod $module;)+

        /// Every tool, sorted by name.
        ///
        /// The spec asks for a deterministic order so a client can cache the
        /// list and compare it cheaply. Sorting here rather than at a call
        /// site is what makes that a property of the collection: there is no
        /// order to add a tool in wrongly.
        pub fn definitions() -> Vec<Definition> {
            let mut tools = vec![$(definition::<$module::$tool>()),+];
            tools.sort_by(|a, b| a.name.cmp(&b.name));
            tools
        }

        /// The definition of one tool by name.
        ///
        /// `rmcp` reads this to validate the SEP-2243 `Mcp-Param-*` headers
        /// against the body, so it has to be the same definition
        /// [`definitions`] returns — which it is, because there is one.
        pub fn definition_of(name: &str) -> Option<Definition> {
            match name {
                $(<$module::$tool as Tool>::NAME => Some(definition::<$module::$tool>()),)+
                _ => None,
            }
        }

        /// Run the tool `name`, or refuse a name that is not one of ours.
        pub async fn call(
            name: &str,
            arguments: Option<JsonObject>,
            ctx: &Ctx,
        ) -> Result<CallToolResult, ErrorData> {
            match name {
                $(<$module::$tool as Tool>::NAME => invoke::<$module::$tool>(arguments, ctx).await,)+
                unknown => Failure::NoSuchTool {
                    name: unknown.to_owned(),
                }
                .respond(),
            }
        }
    };
}

// Every tool this server offers.
//
// One line per tool: the module, and the type inside it that implements
// `Tool`. The macro declares the module, so there is no second list to keep
// in step and no way to register a tool in the wrong place — the sort is a
// property of the collection rather than a call at the end of a builder.
tools! {
    resolve_archive_url::ResolveArchiveUrl,
}

/// What a tool is allowed to reach.
///
/// Built once per request by the service factory in [`crate::router`] and
/// handed to every handler, so that shared state is cloned in rather than
/// rebuilt per call or stored somewhere that has to outlive an invocation.
///
/// Empty today, deliberately. What fills it is the seams that carry state a
/// request needs and a handler should not build: `archive` (#10) and `store`
/// (#20). [`crate::registry`] and [`crate::page`] are not among them — both
/// are pure, so a tool reaches them as modules and there is nothing to hand
/// it. A tool reaching for anything that is neither here nor a pure module
/// has gone around a seam.
#[derive(Debug, Clone, Default)]
#[non_exhaustive]
pub struct Ctx;

impl Ctx {
    pub fn new() -> Self {
        Self
    }
}

/// One MCP tool: what a client is told, and what runs.
///
/// Implemented by a unit struct in a module of its own. Everything a client
/// sees is derived from the associated items, so there is nothing to keep in
/// step by hand.
pub trait Tool {
    /// The name a client calls. Snake case, verb first, because it is what a
    /// model reads in a list of twenty.
    const NAME: &'static str;

    /// The title a client shows a person, where it shows one.
    const TITLE: &'static str;

    /// What this tool does, written for a model choosing between tools
    /// without documentation to read. See #23.
    const DESCRIPTION: &'static str;

    /// True when calling this changes nothing. Every tool here reads.
    const READ_ONLY: bool;

    /// True when the same arguments always produce the same answer.
    const IDEMPOTENT: bool;

    /// True when the arguments name something outside this server — a
    /// package on a registry, rather than an identifier this server minted.
    const OPEN_WORLD: bool;

    /// What the handler is given. The input schema is generated from it, so
    /// the schema and the handler cannot disagree.
    type Args: DeserializeOwned + JsonSchema + Send + 'static;

    /// What the handler returns. The output schema and the
    /// `structuredContent` of the answer are both generated from it.
    type Output: Serialize + JsonSchema + 'static;

    /// Run the tool.
    ///
    /// The error type is [`Failure`] rather than anything of MCP's, which is
    /// what keeps a handler from putting a failure on the wrong channel:
    /// [`Failure::respond`] decides that once, in [`invoke`], for every tool.
    fn call(args: Self::Args, ctx: &Ctx) -> impl Future<Output = Result<Self::Output, Failure>>;
}

/// The definition of `T`, as `tools/list` returns it.
fn definition<T: Tool>() -> Definition {
    Definition::new(T::NAME, T::DESCRIPTION, Arc::new(JsonObject::new()))
        .with_title(T::TITLE)
        .with_input_schema::<T::Args>()
        .with_output_schema::<T::Output>()
        .with_annotations(
            ToolAnnotations::default()
                .read_only(T::READ_ONLY)
                .idempotent(T::IDEMPOTENT)
                .open_world(T::OPEN_WORLD),
        )
}

/// Deserialize `arguments`, run `T`, and put the answer or the failure on the
/// channel it belongs to.
///
/// This is where every tool's error handling happens, which is why a handler
/// has none of its own. Arguments that do not validate are the client's
/// mistake and take the protocol channel; anything that goes wrong afterwards
/// is [`Failure`]'s to place.
async fn invoke<T: Tool>(
    arguments: Option<JsonObject>,
    ctx: &Ctx,
) -> Result<CallToolResult, ErrorData> {
    let args = match serde_json::from_value::<T::Args>(arguments.unwrap_or_default().into()) {
        Ok(args) => args,
        Err(invalid) => {
            return Failure::InvalidParams {
                message: invalid.to_string(),
            }
            .respond()
        }
    };

    let output = match T::call(args, ctx).await {
        Ok(output) => output,
        Err(failure) => return failure.respond(),
    };

    // A tool whose own output will not serialise is a bug in this crate, not
    // something the caller did, so it takes the internal channel rather than
    // being reported as the tool failing.
    match serde_json::to_value(output) {
        Ok(value) => Ok(CallToolResult::structured(value)),
        Err(_) => Failure::Internal {
            doing: "answering a tool call",
        }
        .respond(),
    }
}
