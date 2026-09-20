//! What this server tells a client it is, and what it can do.
//!
//! [`Diffpack`] is the MCP handler: one value, built fresh for every request,
//! answering `server/discover` and `tools/list` today and the nineteen tools
//! of phases 3 and 4 as they land. [`transport_config`] is how that handler is
//! exposed over Streamable HTTP; [`crate::router`] puts the two together.
//!
//! # Why there is no session
//!
//! Revision `2026-07-28` removed protocol-level sessions and the `initialize`
//! handshake: every request carries its own protocol version and capabilities,
//! and `server/discover` replaced the handshake. That is not a constraint here
//! so much as a description — a Vercel function has no warm process to hold a
//! session in, so a session id would be a promise the deployment could not
//! keep. Clients on older revisions are served statelessly too, for the same
//! reason: there is nowhere for their session to live either.

use std::borrow::Cow;
use std::future::Future;
use std::panic::AssertUnwindSafe;

use futures::FutureExt as _;
use rmcp::model::{
    CacheScope, CallToolRequestParams, CallToolResponse, Implementation, ListToolsResult,
    PaginatedRequestParams, ProtocolVersion, ServerCapabilities, ServerConfig, Tool,
};
use rmcp::service::RequestContext;
use rmcp::transport::streamable_http_server::StreamableHttpServerConfig;
use rmcp::{ErrorData, RoleServer, ServerHandler};

/// The name a client shows a user for this server.
///
/// Not `diffpack-server`: the crate is the server, but what a user picked in
/// their client is the product. The version beside it is the crate's, so the
/// identity names the build that answered — the same reasoning as
/// [`crate::health`].
pub const NAME: &str = "diffpack";

/// How long a client may treat a `tools/list` answer as fresh.
///
/// The tool list changes only when a new build deploys, so an hour of
/// staleness costs a client nothing and saves it a round trip — which on a
/// serverless function is a round trip that might have paid for a cold start.
pub const TOOL_LIST_TTL_MS: u64 = 60 * 60 * 1000;

/// The MCP server.
///
/// Built by the service factory on every request, so it holds nothing that
/// outlives one: shared state (the HTTP client, the Blob client of #20) is
/// captured by the factory closure and cloned in, not stored here.
#[derive(Debug, Clone, Default)]
pub struct Diffpack;

impl Diffpack {
    pub fn new() -> Self {
        Self
    }

    /// Every tool this server offers, in the order `tools/list` returns them.
    ///
    /// Sorted by name rather than listed in whatever order the tools were
    /// written. The spec asks for a deterministic order so a client can cache
    /// the list and compare it cheaply, and sorting is the only order that
    /// stays deterministic when someone adds a tool in the middle of the file.
    ///
    /// Empty until #11. The sort is here now so that the first tool cannot
    /// arrive without it.
    fn tools() -> Vec<Tool> {
        let mut tools: Vec<Tool> = Vec::new();
        tools.sort_by(|a, b| a.name.cmp(&b.name));
        tools
    }
}

impl ServerHandler for Diffpack {
    /// Identity, capabilities and the one paragraph an agent reads before it
    /// decides whether this server is worth calling.
    ///
    /// `server/discover` is built from this and from
    /// [`Self::supported_protocol_versions`], so there is one place to change
    /// when either moves.
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new(NAME, env!("CARGO_PKG_VERSION")))
            .with_instructions(
                "Diff two versions of a published package and read the result. \
                 Works with npm, crates.io and PyPI. Start with `tools/list`: \
                 every tool takes a registry, a package name and a version, and \
                 names the registry the way that registry does.",
            )
    }

    /// Every revision the SDK knows, oldest first.
    ///
    /// Deliberately not narrowed to the current one. The clients that matter
    /// are not all on `2026-07-28`, this server holds no state that an older
    /// revision's session rules could contradict, and a client turned away
    /// here has no fallback — there is only the one endpoint.
    fn supported_protocol_versions(&self) -> Cow<'static, [ProtocolVersion]> {
        Cow::Borrowed(ProtocolVersion::KNOWN_VERSIONS)
    }

    /// The tool list, with the freshness a client needs to avoid asking again.
    ///
    /// `cacheScope` is public because this server has no authorization
    /// contexts to keep apart: every caller is anonymous and every caller gets
    /// the same list, so an intermediary caching one answer for everyone is
    /// correct rather than a leak.
    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, ErrorData> {
        Ok(ListToolsResult::with_all_items(Self::tools())
            .with_ttl_ms(TOOL_LIST_TTL_MS)
            .with_cache_scope(CacheScope::Public))
    }
}

/// A handler whose panics become JSON-RPC errors instead of silence.
///
/// # Why this cannot be an HTTP layer
///
/// The obvious place for a panic guard is a `tower` layer on the HTTP stack,
/// and it is the wrong place. `rmcp` runs the handler on a task of its own
/// (`tokio::spawn`, in its Streamable HTTP service), so a panic unwinds that
/// task and never passes through the tower stack at all. The HTTP side is
/// left awaiting a response that will never be sent: the request does not
/// fail, it *hangs*, until Vercel kills the function at `maxDuration` and the
/// client is left with a dropped connection and no idea which call did it.
///
/// That is strictly worse than the `500` #7 set out to prevent, so the guard
/// has to be inside the handler, above the spawn. Here the panic is still a
/// value, the request id is still in hand, and the answer is an ordinary
/// JSON-RPC internal error that a client can render and a caller can report.
///
/// The panic's own message is deliberately not forwarded. It is written for
/// us, it can name a file in this repository, and the caller can do nothing
/// with it — #26 is where it reaches a log instead.
#[derive(Debug, Clone)]
pub struct Guarded<S>(pub S);

impl<S: ServerHandler> ServerHandler for Guarded<S> {
    fn get_info(&self) -> ServerConfig {
        self.0.get_info()
    }

    fn supported_protocol_versions(&self) -> Cow<'static, [ProtocolVersion]> {
        self.0.supported_protocol_versions()
    }

    async fn list_tools(
        &self,
        request: Option<PaginatedRequestParams>,
        context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, ErrorData> {
        guard("listing tools", self.0.list_tools(request, context)).await
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, ErrorData> {
        // The one that matters. Every tool of phases 3 and 4 runs through
        // here, against archives from registries we do not control, and an
        // index that is off by one on a malformed file is a panic like any
        // other.
        guard("calling a tool", self.0.call_tool(request, context)).await
    }
}

/// Run `handler`, turning a panic into an internal error.
///
/// `AssertUnwindSafe` is the honest annotation rather than a way around the
/// check: nothing here is shared across the catch. The handler is built fresh
/// per request and dropped after it, so there is no state left half-updated
/// for a later request to observe.
async fn guard<T>(
    doing: &'static str,
    handler: impl Future<Output = Result<T, ErrorData>>,
) -> Result<T, ErrorData> {
    AssertUnwindSafe(handler)
        .catch_unwind()
        .await
        .unwrap_or_else(|_| {
            Err(ErrorData::internal_error(
                format!("diffpack failed while {doing}."),
                None,
            ))
        })
}

/// How the handler is exposed over Streamable HTTP.
///
/// `allowed_origins` is the browser origins permitted to drive this server;
/// see [`crate::router::router`] for where it comes from.
pub fn transport_config(allowed_origins: Vec<String>) -> StreamableHttpServerConfig {
    StreamableHttpServerConfig::default()
        // No sessions, for any client. The default keeps them for revisions
        // before `2026-07-28`, which would mean minting an id in one function
        // invocation and looking it up in another that has never heard of it.
        .with_legacy_session_mode(false)
        // An ordinary tool call answers with one `application/json` body
        // rather than a one-message SSE stream. A stream costs a client a
        // parser it does not need here, and buys nothing a function that
        // answers once can deliver. The transport still falls back to SSE if a
        // handler emits a notification before its result, so nothing is lost.
        .with_json_response(true)
        // `Host` validation is rmcp's DNS-rebinding defence, and it is aimed
        // at servers running on a developer's own machine: reject a `Host`
        // that is not loopback and a rebound name cannot reach them. On a
        // public deployment it defends nothing — an attacker's page can send
        // the correct `Host` simply by fetching the real URL — while the
        // default loopback list would reject every production and preview
        // deployment. Turned off deliberately; `Origin` below is the defence
        // that does apply.
        .disable_allowed_hosts()
        // Enforced even when the list is empty, which is the point: with no
        // entries every request that carries an `Origin` is refused, and a
        // request with none is served. That is exactly the shape of the
        // clients this server has — local processes, no browser — so the
        // default is the closed one, and opening it is a deployment decision.
        .enforce_origin_validation()
        .with_allowed_origins(allowed_origins)
}
