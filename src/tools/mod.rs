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
//! # Who reads a description
//!
//! Every `description` in a tool's schema reaches a model: [`Tool::DESCRIPTION`],
//! and one per field of [`Tool::Args`] and [`Tool::Output`] that carries a doc
//! comment. Those are written for that reader and name nothing in this
//! repository — a model told a field's enum "comes from [`crate::registry`]"
//! has been handed our reasoning rather than something it can act on. Why a
//! field is the shape it is goes in an ordinary `//` comment beside it, or in
//! the tool module's own header. `tests/tools.rs` holds every tool to this,
//! over the whole collection rather than tool by tool, so the rule arrives
//! before the tool that would have broken it (#23).
//!
//! The doc comment on an `Args` or `Output` struct itself is the exception and
//! goes the other way: `rmcp` strips the root `title` and `description` off
//! both schemas as noise, so a sentence written there reaches nobody. It is a
//! note to the next reader of the file, and a rule a caller has to know cannot
//! live in one.
//!
//! # What a tool may reach
//!
//! [`Ctx`], and nothing else it has to build itself.
//! `scripts/check-tool-seams.sh` enforces the rest: a module here cannot name
//! an HTTP client or the blob store. See
//! [`docs/architecture.md`](../../docs/architecture.md) and ADR 0002.

use std::future::Future;
use std::path::Path;
use std::sync::Arc;

use rmcp::model::{CallToolResult, JsonObject, Tool as Definition, ToolAnnotations};
use rmcp::ErrorData;
use schemars::JsonSchema;
use serde::de::DeserializeOwned;
use serde::Serialize;

use crate::archive::Archive;
use crate::catalogue::Catalogue;
use crate::error::Failure;

/// Declare the tools, and build the collection and the dispatch from one list.
///
/// `$module::$type` both names the file and the implementation inside it, so
/// a module cannot be registered under another module's name and a module
/// that is never registered is never compiled in.
macro_rules! tools {
    ($($module:ident::$tool:ident),+ $(,)?) => {
        $(pub mod $module;)+

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
    diff_package_versions::DiffPackageVersions,
    get_file_content::GetFileContent,
    list_package_files::ListPackageFiles,
    list_package_versions::ListPackageVersions,
    resolve_archive_url::ResolveArchiveUrl,
}

/// What a tool is allowed to reach.
///
/// Built once per request by the service factory in [`crate::router`] and
/// handed to every handler, so that shared state is cloned in rather than
/// rebuilt per call or stored somewhere that has to outlive an invocation.
///
/// Two seams today — [`Archive`], which arrived with #11, the first tool that
/// reads a package's files, and [`Catalogue`], which arrived with #18, the
/// first that reads what a package has released — and `store` (#20) beside
/// them when there is a cached result to reach for. [`crate::registry`] and
/// [`crate::page`] are not among them and do not need to be: both are pure,
/// so a tool reaches them as modules and there is nothing to hand it. A tool
/// reaching for anything that is neither here nor a pure module has gone
/// around a seam.
///
/// Each is behind an [`Arc`] because this is cloned into every handler and an
/// adapter is not free to rebuild: a live one shares the process's HTTP
/// client and a fixture one is a path it reads from.
///
/// # Two constructors, and why there is no third
///
/// [`Ctx::new`] is every seam live and [`Ctx::fixture`] is every seam reading
/// from disk. Both name every field, so adding a seam is a compile error in
/// each of them and its author answers for both worlds at once.
///
/// What they replace is a builder per seam — `with_archive(archive)` filling
/// the rest from `Self::new()`. Naming one seam there left the others live,
/// so a test that reached a seam it had not named went to a registry. That is
/// a passing test until the day it is a flake, and the flake names the
/// registry rather than the context that let it be asked.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct Ctx {
    archive: Arc<Archive>,
    catalogue: Arc<Catalogue>,
}

impl Ctx {
    /// What production hands a handler: both seams reaching the registries.
    pub fn new() -> Self {
        Self {
            archive: Arc::new(Archive::live()),
            catalogue: Arc::new(Catalogue::live()),
        }
    }

    /// What the suite hands a handler: both seams reading from `fixtures`.
    ///
    /// `fixtures` is the root the checked-in sets live under, and each seam
    /// is given its own directory inside it. One argument rather than one per
    /// seam because the suite has no use for mixing sets — what it needs is
    /// that nothing it builds can reach a registry, which a constructor that
    /// mentions no live adapter is.
    ///
    /// `router_with` takes the service factory that builds this, so a test
    /// reaches the fixture adapters through the path production takes rather
    /// than around it.
    pub fn fixture(fixtures: impl AsRef<Path>) -> Self {
        let fixtures = fixtures.as_ref();
        Self {
            archive: Arc::new(Archive::fixture(fixtures.join("archives"))),
            catalogue: Arc::new(Catalogue::fixture(fixtures.join("versions"))),
        }
    }

    /// A version's files.
    pub fn archive(&self) -> &Archive {
        &self.archive
    }

    /// What a package has released.
    pub fn catalogue(&self) -> &Catalogue {
        &self.catalogue
    }
}

impl Default for Ctx {
    fn default() -> Self {
        Self::new()
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
