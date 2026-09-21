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
use std::sync::Arc;
use std::time::Instant;

use rmcp::model::{CallToolResult, JsonObject, Tool as Definition, ToolAnnotations};
use rmcp::ErrorData;
use schemars::JsonSchema;
use serde::de::DeserializeOwned;
use serde::Serialize;

use crate::archive::{Archive, FileMap};
use crate::error::Failure;
use crate::log::{Record, Sink, Spent};
use crate::registry::Registry;

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
        ///
        /// Private, and reached only through [`call`], which is what makes
        /// the log line a property of every dispatch rather than of the
        /// tools that remembered to write one.
        async fn dispatch(
            name: &str,
            arguments: Option<JsonObject>,
            ctx: &Ctx,
        ) -> Result<CallToolResult, Failure> {
            match name {
                $(<$module::$tool as Tool>::NAME => invoke::<$module::$tool>(arguments, ctx).await,)+
                unknown => Err(Failure::NoSuchTool {
                    name: unknown.to_owned(),
                }),
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
    resolve_archive_url::ResolveArchiveUrl,
}

/// What a tool is allowed to reach.
///
/// Built once per request by the service factory in [`crate::router`] and
/// handed to every handler, so that shared state is cloned in rather than
/// rebuilt per call or stored somewhere that has to outlive an invocation.
///
/// One seam today — [`Archive`], which arrived with #11, the first tool that
/// reads a package's files — and `store` (#20) beside it when there is a
/// cached result to reach for. [`crate::registry`] and [`crate::page`] are
/// not among them and do not need to be: both are pure, so a tool reaches
/// them as modules and there is nothing to hand it. A tool reaching for
/// anything that is neither here nor a pure module has gone around a seam.
///
/// The archive is behind an [`Arc`] because this is cloned into every
/// handler and an adapter is not free to rebuild: the live one is the
/// process's HTTP client and the fixture one is a path it reads from.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct Ctx {
    archive: Arc<Archive>,
    log: Sink,

    /// Where this request's time has gone so far. Behind an [`Arc`] because
    /// a `Ctx` is cloned into every handler and the phases they spend have
    /// to add up to one call's.
    spent: Arc<Spent>,
}

impl Ctx {
    /// What production hands a handler: archives from the registries.
    pub fn new() -> Self {
        Self::with_archive(Archive::live())
    }

    /// The same, with the archive source supplied.
    ///
    /// The seam the suite drives. `router_with` takes the service factory
    /// that builds this, so a test reaches the fixture adapter through the
    /// path production takes rather than around it.
    pub fn with_archive(archive: Archive) -> Self {
        Self {
            archive: Arc::new(archive),
            log: Sink::default(),
            spent: Arc::new(Spent::default()),
        }
    }

    /// The same, writing its log lines to `log`.
    ///
    /// The other half of the seam `tests/log.rs` drives: production writes to
    /// the runtime logs and the suite writes to a buffer it can read back,
    /// through the same factory and the same [`call`].
    pub fn logging_to(self, log: Sink) -> Self {
        Self { log, ..self }
    }

    /// A version's files.
    ///
    /// Returns the seam with a stopwatch on it rather than the seam itself,
    /// so that there is no way to read an archive that is not counted. A
    /// handler is unchanged by it: `ctx.archive().fetch(..)` is the same
    /// call it always was.
    pub fn archive(&self) -> Timed<'_> {
        Timed {
            archive: &self.archive,
            spent: &self.spent,
        }
    }
}

/// The archive seam, with the time a fetch takes recorded.
///
/// The same shape as [`crate::mcp::Guarded`]: a wrapper that adds one
/// property to something a caller already knows how to use, so that the
/// property is not a thing each caller has to remember.
#[derive(Debug)]
pub struct Timed<'a> {
    archive: &'a Archive,
    spent: &'a Spent,
}

impl Timed<'_> {
    /// The files in `version` of `package`, and the time it took on the
    /// request's tally.
    pub async fn fetch(
        &self,
        registry: Registry,
        package: &str,
        version: &str,
    ) -> Result<FileMap, Failure> {
        let started = Instant::now();
        let files = self.archive.fetch(registry, package, version).await;

        // Recorded whether or not it worked. A registry that times out is
        // exactly the call worth knowing the fetch time of.
        self.spent.fetching(started.elapsed());
        files
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
    /// [`call`] decides that once, for every tool, after the line describing
    /// the call has been written.
    fn call(args: Self::Args, ctx: &Ctx) -> impl Future<Output = Result<Self::Output, Failure>>;
}

/// Run the tool `name`, and leave one line behind saying what happened.
///
/// The line is written here rather than in a handler because "every tool call
/// emits one" is a property of the dispatch: a tool that forgot would not
/// fail to compile, and the gap would be invisible until the call nobody
/// logged was the one being looked for.
pub async fn call(
    name: &str,
    arguments: Option<JsonObject>,
    ctx: &Ctx,
) -> Result<CallToolResult, ErrorData> {
    // Summarised before the dispatch, because the dispatch consumes them.
    let record = Record::new(name).about(arguments.as_ref());

    let started = Instant::now();
    let answer = dispatch(name, arguments, ctx).await;
    ctx.log
        .write(&record.taking(started.elapsed(), &ctx.spent).ending(&answer));

    // The one place a `Failure` is put on its channel. Every path into this
    // function returns one, so there is no arm that can answer without
    // having been described a line earlier.
    match answer {
        Ok(result) => Ok(result),
        Err(failure) => failure.respond(),
    }
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

/// Deserialize `arguments` and run `T`.
///
/// This is where every tool's error handling happens, which is why a handler
/// has none of its own. Everything that can go wrong is a [`Failure`],
/// including arguments that did not validate and an answer that would not
/// serialise — which is what lets [`call`] name the outcome in a line before
/// [`Failure::respond`] decides which channel it leaves on.
async fn invoke<T: Tool>(
    arguments: Option<JsonObject>,
    ctx: &Ctx,
) -> Result<CallToolResult, Failure> {
    let args = serde_json::from_value::<T::Args>(arguments.unwrap_or_default().into()).map_err(
        |invalid| Failure::InvalidParams {
            message: invalid.to_string(),
        },
    )?;

    let output = T::call(args, ctx).await?;

    // A tool whose own output will not serialise is a bug in this crate, not
    // something the caller did, so it takes the internal channel rather than
    // being reported as the tool failing.
    serde_json::to_value(output)
        .map(CallToolResult::structured)
        .map_err(|_| Failure::Internal {
            doing: "answering a tool call",
        })
}
