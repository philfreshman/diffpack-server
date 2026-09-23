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
//! The [`Call`] it is handed, and nothing else it has to build itself.
//! `scripts/check-tool-seams.sh` enforces the rest: a module here cannot name
//! an HTTP client or the blob store. See
//! [`docs/architecture.md`](../../docs/architecture.md) and ADR 0002.

use std::future::Future;
use std::path::Path;
use std::sync::Arc;
use std::time::Instant;

use rmcp::model::{
    CallToolResult, ContentBlock, JsonObject, Resource, Tool as Definition, ToolAnnotations,
};
use rmcp::ErrorData;
use schemars::JsonSchema;
use serde::de::DeserializeOwned;
use serde::Serialize;

use crate::archive::{Archive, FileMap};
use crate::cache_key::DiffKey;
use crate::catalogue::Catalogue;
use crate::error::Failure;
use crate::log::{Line, Lookup, Sink, Spent};
use crate::registry::{Hit, Registry, Versions};
use crate::search::Search;
use crate::store::{DiffStore, Entry, Memory};

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
            call: &Call,
        ) -> Result<CallToolResult, Failure> {
            match name {
                $(<$module::$tool as Tool>::NAME => invoke::<$module::$tool>(arguments, call).await,)+
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
    get_diff_tree::GetDiffTree,
    get_file_content::GetFileContent,
    get_file_diff::GetFileDiff,
    list_package_files::ListPackageFiles,
    list_package_versions::ListPackageVersions,
    resolve_archive_url::ResolveArchiveUrl,
    search_packages::SearchPackages,
}

/// What a tool is allowed to reach.
///
/// Built once per request by the service factory in [`crate::router`], so
/// that shared state is cloned in rather than rebuilt per call or stored
/// somewhere that has to outlive an invocation. A handler is not handed this
/// but a [`Call`] made from it, which is where the one call's tally lives.
///
/// Four seams today — [`Archive`], which arrived with #11, the first tool
/// that reads a package's files, [`Catalogue`], which arrived with #18, the
/// first that reads what a package has released, [`Search`], which
/// arrived with #19, the first that asks a registry which packages it has,
/// and [`DiffStore`], which arrived with #21, the first cached result to
/// reach for. [`crate::registry`] and
/// [`crate::page`] are not among them and do not need to be: both are pure,
/// so a tool reaches them as modules and there is nothing to hand it. A tool
/// reaching for anything that is neither here nor a pure module has gone
/// around a seam.
///
/// Beside the seams it carries the [`Sink`] the one line per call is written
/// to, which a handler never touches. That is all: everything here outlives
/// a call, so cloning a `Ctx` shares nothing any call has written. What one
/// call spent and what it found in the store is the [`Call`]'s, made when
/// the call starts (#96).
///
/// Each seam is behind an [`Arc`] because this is cloned into every call
/// and an adapter is not free to rebuild: a live one shares the process's
/// HTTP client and a fixture one is a path it reads from.
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
///
/// [`Ctx::logging_to`] is not a third, and the difference is the whole rule:
/// it spreads `..self`, so it changes a context that has already chosen its
/// world. A spread of `..Self::new()` is what reopens this, whatever it is
/// called.
///
/// What the compiler checks there is that every field was answered for, not
/// that the answer was a fixture one. [`Ctx::seams`] is the half it cannot
/// check: it names the seams by taking this struct apart, so a field added
/// here has to be called a seam or not, and `tests/ctx.rs` holds its own list
/// of calls to whatever that answer was.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct Ctx {
    archive: Arc<Archive>,
    catalogue: Arc<Catalogue>,
    search: Arc<Search>,
    store: Arc<DiffStore>,
    log: Sink,
}

impl Ctx {
    /// What production builds: every seam reaching the registries.
    pub fn new() -> Self {
        Self {
            archive: Arc::new(Archive::live()),
            catalogue: Arc::new(Catalogue::live()),
            search: Arc::new(Search::live()),
            store: Arc::new(DiffStore::live()),
            log: Sink::default(),
        }
    }

    /// What the suite builds: every seam reading from `fixtures`.
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
    ///
    /// The store is the one seam with no directory under `fixtures`, and
    /// nothing is wrong with that: a cached result is this server's own
    /// answer rather than a registry's document, so there is nothing to
    /// check in. What stands in for the fixture set is a store that keeps
    /// its blobs in this process — reaching no further than the others do,
    /// and readable the way a checked-in set is by the suite that wants to
    /// see what was written ([`Memory`]).
    ///
    /// The log is the same here as in production: it is not a seam, and a
    /// suite that wants the lines back asks for them with
    /// [`Ctx::logging_to`].
    pub fn fixture(fixtures: impl AsRef<Path>) -> Self {
        let fixtures = fixtures.as_ref();
        Self {
            archive: Arc::new(Archive::fixture(fixtures.join("archives"))),
            catalogue: Arc::new(Catalogue::fixture(fixtures.join("versions"))),
            search: Arc::new(Search::fixture(fixtures.join("searches"))),
            store: Arc::new(Memory::new().store()),
            log: Sink::default(),
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

    /// The same, caching its results in `store`.
    ///
    /// The third of the same shape and not a builder: it spreads `..self`,
    /// so it changes a context that has already chosen its world rather than
    /// filling in a seam it was not given. What it is for is the two things
    /// a store can be that a fixture set cannot — not there, and slow — and
    /// neither is reachable by pointing a constructor at a directory.
    pub fn storing_in(self, store: DiffStore) -> Self {
        Self {
            store: Arc::new(store),
            ..self
        }
    }

    /// The seams this carries, by name.
    ///
    /// Taking `Self` apart is the point of the first line. It names every
    /// field and spreads nothing, so a seam added to this struct does not
    /// compile until somebody has said whether it is one — and `tests/ctx.rs`
    /// answers this rather than a list of its own, so a seam nobody wrote a
    /// call for fails there instead of going unasserted until the day it is
    /// live.
    ///
    /// The log is not a seam. Nothing outside this process is behind it,
    /// which is the whole of what a seam is here.
    pub fn seams(&self) -> &'static [&'static str] {
        let Self {
            archive: _,
            catalogue: _,
            search: _,
            store: _,
            log: _,
        } = self;

        &["archive", "catalogue", "search", "store"]
    }
}

impl Default for Ctx {
    fn default() -> Self {
        Self::new()
    }
}

/// One call, while it runs: a [`Ctx`]'s seams, and the tally they write into.
///
/// What a handler is handed. The seams are the context's, and the tally is
/// this call's alone: the [`Spent`] its phases add up in, and the [`Lookup`]
/// saying what it found in the store. Both are made with the call, which is
/// what makes the line it leaves behind about this call and no other.
///
/// # Why the tally is not in the Ctx
///
/// It was, until #96, and nothing in production could tell: [ADR
/// 0008](../../docs/adr/0008-no-sessions.md) means a request is one call and
/// the factory in [`crate::router`] builds a `Ctx` per request, so a context
/// and a call lived exactly as long. A `Ctx` cloned across two calls is where
/// they came apart — a lookup keeps a hit once it has one, so every call
/// after the first hit said `hit`, and a window measured from the context's
/// birth put the first call's fetches in the second's. Held here, the tally
/// cannot be shared: a `Call` is not `Clone`, a handler only borrows one, and
/// [`Call::new`] is the only way to make one, with a tally of its own.
///
/// The context is cloned in rather than borrowed so that a handler's
/// signature names one type and no lifetime. It is four [`Arc`]s and a
/// [`Sink`], and cloning it is safe now that it holds nothing a call writes.
#[derive(Debug)]
pub struct Call {
    ctx: Ctx,

    /// Where this call's time has gone so far.
    spent: Spent,

    /// What this call found in the store.
    lookup: Lookup,
}

impl Call {
    /// A call starting now, reaching `ctx`'s seams.
    ///
    /// Public for the two places that run a handler without writing a line:
    /// [`crate::mcp`]'s resource read, which writes none until #26 says what
    /// it should hold, and the suites that call a handler directly. A call
    /// that leaves a line behind is made by `run`, beside [`call`].
    pub fn new(ctx: &Ctx) -> Self {
        Self {
            ctx: ctx.clone(),
            spent: Spent::new(),
            lookup: Lookup::default(),
        }
    }

    /// Cached diff results.
    ///
    /// Not timed, and that is the difference rather than an omission. The
    /// `fetch` phase answers how long a call waited on a *registry*, and a
    /// cache read that counted towards it would report the call that avoided
    /// two downloads as the one that waited longest. What is recorded
    /// instead is what the lookup found, which is the other question an
    /// operator asks of a slow call.
    ///
    /// Returns the seam with that recording attached rather than the seam
    /// itself, for the reason [`Call::archive`] does: a handler is unchanged
    /// by it — `call.store().get(..)` is the same call it always was — and
    /// there is no way left to read the store without the line saying what
    /// came back.
    pub fn store(&self) -> Recorded<'_> {
        Recorded {
            store: &self.ctx.store,
            lookup: &self.lookup,
        }
    }

    /// A version's files.
    ///
    /// Returns the seam with a stopwatch on it rather than the seam itself,
    /// so that there is no way to read an archive that is not counted. A
    /// handler is unchanged by it: `call.archive().fetch(..)` is the same
    /// call it always was.
    pub fn archive(&self) -> Timed<'_, Archive> {
        self.timed(&self.ctx.archive)
    }

    /// What a package has released.
    ///
    /// Timed for the same reason and by the same wrapper. Reading a
    /// catalogue is a request to a registry, and a `fetch` phase that only
    /// counted archives would report the one tool that does nothing else as
    /// a call that waited on nobody — which is the reading an operator makes
    /// when the phase is absent.
    pub fn catalogue(&self) -> Timed<'_, Catalogue> {
        self.timed(&self.ctx.catalogue)
    }

    /// Which packages a registry has.
    ///
    /// Timed like the other two. A search is one request to a registry and
    /// sometimes none — the source that is a whole index is held between
    /// invocations — so the phase is what says which of the two this call
    /// was, and a search left uncounted would read as a call that waited on
    /// nobody either way.
    pub fn search(&self) -> Timed<'_, Search> {
        self.timed(&self.ctx.search)
    }

    /// `seam`, with this call's tally attached.
    fn timed<'a, S>(&'a self, seam: &'a S) -> Timed<'a, S> {
        Timed {
            seam,
            spent: &self.spent,
        }
    }
}

/// A seam that waits on a registry, with that wait recorded.
///
/// The same shape as [`crate::mcp::Guarded`]: a wrapper that adds one
/// property to something a caller already knows how to use, so that the
/// property is not a thing each caller has to remember. One wrapper over both
/// seams rather than one each, because "how long did this call wait on a
/// registry" is a question about the call and not about which of them
/// answered it — two wrappers would be two places for that to drift.
///
/// `Copy`, and each method takes it by value, because of how the one tool
/// that reads two archives asks for them:
///
/// ```ignore
/// try_join!(
///     call.archive().fetch(registry, &package, &from),
///     call.archive().fetch(registry, &package, &to),
/// )
/// ```
///
/// Each `call.archive()` there is a temporary that the statement drops while
/// the futures are still running. Taken by reference, the borrow outlives
/// what it borrows and the tool does not compile; moved into the future, it
/// is two pointers that go where the work goes. The alternative was a `let`
/// binding at each such call site, which is a thing to remember at the one
/// place this seam is used concurrently — and the seam exists so that
/// counting a fetch is not a thing to remember.
#[derive(Debug)]
pub struct Timed<'a, S> {
    seam: &'a S,
    spent: &'a Spent,
}

// Derived, these would ask the seam behind the reference to be `Copy` too,
// which neither adapter is and neither needs to be: what is copied is two
// pointers.
impl<S> Clone for Timed<'_, S> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<S> Copy for Timed<'_, S> {}

/// The store, with what a lookup found written on the call.
///
/// The same shape as [`Timed`] and for the same reason, but not the same
/// wrapper: what is recorded here is not a duration. A cache read is
/// deliberately outside the fetch phase, so a seam that shared `Timed` would
/// have to be given a stopwatch it must not start.
#[derive(Debug, Clone, Copy)]
pub struct Recorded<'a> {
    store: &'a Arc<DiffStore>,
    lookup: &'a Lookup,
}

impl Recorded<'_> {
    /// The entry for `key`, and what that lookup found on the call's line.
    ///
    /// A store that is not there is recorded as that rather than as the miss
    /// it looks like from here. Every `get` on one answers `None`, so a
    /// deployment with no credentials would otherwise leave a line saying
    /// `miss` for every call it ever serves — a hundred percent miss rate,
    /// which is what a cache that is working and cold reads as too.
    pub async fn get(self, key: &DiffKey) -> Option<Entry> {
        let entry = self.store.get(key).await;

        match self.store.is_available() {
            true => self.lookup.looked(entry.is_some()),
            false => self.lookup.found_no_store(),
        }

        entry
    }

    /// Remember `entry`, after the answer has already gone.
    ///
    /// The handle is cloned here rather than by a caller, because writing an
    /// entry outlives the call that produced it: the work goes to the
    /// runtime's `waitUntil` and the call it came from is gone by the time
    /// it runs.
    pub fn put(self, entry: Entry) {
        Arc::clone(self.store).put(entry);
    }
}

impl Timed<'_, Archive> {
    /// The files in `version` of `package`, and the time it took on the
    /// call's tally.
    pub async fn fetch(
        self,
        registry: Registry,
        package: &str,
        version: &str,
    ) -> Result<FileMap, Failure> {
        self.spent
            .while_fetching(self.seam.fetch(registry, package, version))
            .await
    }
}

impl Timed<'_, Catalogue> {
    /// Every published version of `package`, newest first, the one the
    /// registry points at, and the time it took on the call's tally.
    pub async fn versions(self, registry: Registry, package: &str) -> Result<Versions, Failure> {
        self.spent
            .while_fetching(self.seam.versions(registry, package))
            .await
    }
}

impl Timed<'_, Search> {
    /// The packages on `registry` that answer to `query`, at most `limit` of
    /// them, and the time it took on the call's tally.
    pub async fn hits(
        self,
        registry: Registry,
        query: &str,
        limit: u32,
    ) -> Result<Vec<Hit>, Failure> {
        self.spent
            .while_fetching(self.seam.hits(registry, query, limit))
            .await
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
    fn call(args: Self::Args, call: &Call) -> impl Future<Output = Result<Self::Output, Failure>>;

    /// What a client can read next, given this answer.
    ///
    /// A `resource_link` per entry, beside the structured content. Empty for
    /// every tool but one: a link is worth carrying where it is the *way on*
    /// from an answer, and `diff_package_versions` is the only tool here
    /// whose answer names something a client did not already have a URI for
    /// (#16). On a page of a tree it would be the same link on every call,
    /// pointing back at the comparison the caller is already holding a handle
    /// to.
    ///
    /// Defaulted, unlike [`Self::DESCRIPTION`] and the three hints, and the
    /// difference is which way being wrong falls. Those are per-tool facts
    /// where both wrong answers are spent on a person, so a tool has to say.
    /// This one has a right answer for a tool that points nowhere, and it is
    /// nothing.
    fn links(_output: &Self::Output) -> Vec<Resource> {
        Vec::new()
    }
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
    let line = Line::new(name).about(arguments.as_ref());

    let answer = run(ctx, line, async |call| {
        dispatch(name, arguments, call).await
    })
    .await;

    // The one place a `Failure` is put on its channel. Every path into this
    // function returns one, so there is no arm that can answer without
    // having been described a line earlier.
    match answer {
        Ok(result) => Ok(result),
        Err(failure) => failure.respond(),
    }
}

/// Run `work` as one call over `ctx`, and write `line` saying what happened.
///
/// The call is made here, so its tally starts when the work does and ends
/// with this function: nothing outside `work` can reach it, and nothing
/// after the line is written can add to it. That is what makes the line's
/// phases and its cache outcome this call's and not a neighbour's.
///
/// One step rather than the three lines [`call`] used to hold, because a
/// tool call is not the only thing that will leave a line. A resource read
/// goes through the same seams and writes none today; #26 decides what its
/// line holds, and this is what it calls when it does, rather than a copy of
/// [`call`] with the dispatch swapped out.
pub(crate) async fn run<T>(
    ctx: &Ctx,
    line: Line,
    work: impl AsyncFnOnce(&Call) -> Result<T, Failure>,
) -> Result<T, Failure> {
    let call = Call::new(ctx);
    let started = Instant::now();

    let answer = work(&call).await;
    ctx.log.write(
        &line
            .taking(started.elapsed(), &call.spent)
            .cached(&call.lookup)
            .ending(&answer),
    );

    answer
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
    call: &Call,
) -> Result<CallToolResult, Failure> {
    let args = serde_json::from_value::<T::Args>(arguments.unwrap_or_default().into()).map_err(
        |invalid| Failure::InvalidParams {
            message: invalid.to_string(),
        },
    )?;

    let output = T::call(args, call).await?;
    let links = T::links(&output);

    // A tool whose own output will not serialise is a bug in this crate, not
    // something the caller did, so it takes the internal channel rather than
    // being reported as the tool failing.
    let mut result = serde_json::to_value(output)
        .map(CallToolResult::structured)
        .map_err(|_| Failure::Internal {
            doing: "answering a tool call",
        })?;

    // After the structured content and the text `CallToolResult::structured`
    // mirrors it into, so a client reading the blocks in order sees the
    // answer before what to read next.
    result
        .content
        .extend(links.into_iter().map(ContentBlock::ResourceLink));

    Ok(result)
}
