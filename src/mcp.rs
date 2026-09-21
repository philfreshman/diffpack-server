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
    CacheScope, CallToolRequestParams, CallToolResponse, CancelTaskParams,
    CancelledNotificationParam, CompleteRequestParams, CompleteResult, CustomNotification,
    CustomRequest, CustomResult, DiscoverResult, GetPromptRequestParams, GetPromptResponse,
    GetTaskParams, GetTaskResult, Implementation, InitializeRequestParams, InitializeResult,
    ListPromptsResult, ListResourceTemplatesResult, ListResourcesResult, ListToolsResult,
    PaginatedRequestParams, ProgressNotificationParam, ProtocolVersion, ReadResourceRequestParams,
    ReadResourceResponse, ServerCapabilities, ServerConfig, SubscribeRequestParams,
    SubscriptionFilter, Tool, UnsubscribeRequestParams, UpdateTaskParams,
};
// Logging was deprecated by SEP-2577, but `set_level` is still on the trait
// and a wrapper that skipped it would answer for the handler it wraps. It is
// forwarded like everything else; the deprecation is rmcp's to carry.
#[expect(
    deprecated,
    reason = "forwarding a deprecated method is still forwarding"
)]
use rmcp::model::SetLevelRequestParams;
use rmcp::service::{NotificationContext, RequestContext, SubscriptionContext};
use rmcp::transport::streamable_http_server::StreamableHttpServerConfig;
use rmcp::{ErrorData, RoleServer, ServerHandler};

use crate::tools::{self, Ctx};

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
/// captured by the factory closure and cloned into [`Ctx`], not stored here.
///
/// It describes no tool. A tool's definition lives with its handler in its
/// own module under [`crate::tools`], and what is left here is identity,
/// capabilities and handing a call to the collection — see ADR 0002.
#[derive(Debug, Clone, Default)]
pub struct Diffpack {
    /// What a tool handler is allowed to reach, for the length of one
    /// request.
    ctx: Ctx,
}

impl Diffpack {
    /// What the deployed function serves: archives from the registries.
    pub fn new() -> Self {
        Self::with_ctx(Ctx::new())
    }

    /// The same handler, with what its tools may reach supplied.
    ///
    /// The other half of the seam `router_with` opens. A factory that builds
    /// this is how the suite puts the fixture archive adapter behind every
    /// tool without any tool knowing which adapter it has.
    pub fn with_ctx(ctx: Ctx) -> Self {
        Self { ctx }
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
        Ok(ListToolsResult::with_all_items(tools::definitions())
            .with_ttl_ms(TOOL_LIST_TTL_MS)
            .with_cache_scope(CacheScope::Public))
    }

    /// One tool's definition, which the transport reads to validate the
    /// SEP-2243 `Mcp-Param-*` headers against the body. The same collection
    /// answers it, so what a header is checked against is what `tools/list`
    /// advertised.
    fn get_tool(&self, name: &str) -> Option<Tool> {
        tools::definition_of(name)
    }

    /// Hand the call to the module that owns that name.
    ///
    /// Every decision about the answer — whether the arguments validate,
    /// which channel a failure takes, what `structuredContent` is built from
    /// — is made once, in [`crate::tools`], for every tool. There is nothing
    /// to add here, and a tool that needed something added here would be a
    /// tool that had escaped the shape.
    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, ErrorData> {
        tools::call(&request.name, request.arguments, &self.ctx)
            .await
            .map(CallToolResponse::from)
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

/// # Why every method is written out
///
/// A wrapper that implements only the methods it cares about is not a
/// wrapper: the ones it leaves out resolve to the *trait's* defaults, not to
/// the handler underneath, and the inner handler is never called. For
/// `resources/list` that default is an empty list, so the failure mode is a
/// successful, wrong answer — a resource this server offers, reported as
/// absent, with nothing anywhere saying so. `tests/errors.rs` pins it.
///
/// `rmcp` has this impl as a macro for its own `Box` and `Arc` wrappers, but
/// it is private to that crate, so the forwarding is spelled out here. The
/// cost is that a method added to `ServerHandler` in a later `rmcp` silently
/// falls back to its default again; the compiler cannot warn about that, so
/// the upgrade is the moment to re-read this impl against the trait.
impl<S: ServerHandler> ServerHandler for Guarded<S> {
    // -----------------------------------------------------------------------
    // Not futures, so there is nothing for `catch_unwind` to wrap. These are
    // read off a handler built moments ago, and a panic in one is a panic in
    // a constant.
    // -----------------------------------------------------------------------

    fn get_info(&self) -> ServerConfig {
        self.0.get_info()
    }

    fn supported_protocol_versions(&self) -> Cow<'static, [ProtocolVersion]> {
        self.0.supported_protocol_versions()
    }

    fn negotiate_initialize(
        &self,
        request: &InitializeRequestParams,
    ) -> Result<InitializeResult, ErrorData> {
        self.0.negotiate_initialize(request)
    }

    fn accepted_subscription_filter(
        &self,
        requested: &SubscriptionFilter,
    ) -> Option<SubscriptionFilter> {
        self.0.accepted_subscription_filter(requested)
    }

    fn get_tool(&self, name: &str) -> Option<Tool> {
        self.0.get_tool(name)
    }

    // -----------------------------------------------------------------------
    // Requests. Each one has a JSON-RPC id waiting on it, so each one can be
    // answered with an error instead of left to hang.
    // -----------------------------------------------------------------------

    async fn ping(&self, context: RequestContext<RoleServer>) -> Result<(), ErrorData> {
        guard("answering a ping", self.0.ping(context)).await
    }

    async fn initialize(
        &self,
        request: InitializeRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<InitializeResult, ErrorData> {
        guard("initializing", self.0.initialize(request, context)).await
    }

    async fn discover(
        &self,
        context: RequestContext<RoleServer>,
    ) -> Result<DiscoverResult, ErrorData> {
        guard("describing this server", self.0.discover(context)).await
    }

    async fn complete(
        &self,
        request: CompleteRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<CompleteResult, ErrorData> {
        guard("completing an argument", self.0.complete(request, context)).await
    }

    #[expect(
        deprecated,
        reason = "the trait still has it, so the wrapper still forwards it"
    )]
    async fn set_level(
        &self,
        request: SetLevelRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<(), ErrorData> {
        guard("setting the log level", self.0.set_level(request, context)).await
    }

    async fn get_prompt(
        &self,
        request: GetPromptRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<GetPromptResponse, ErrorData> {
        guard("reading a prompt", self.0.get_prompt(request, context)).await
    }

    async fn list_prompts(
        &self,
        request: Option<PaginatedRequestParams>,
        context: RequestContext<RoleServer>,
    ) -> Result<ListPromptsResult, ErrorData> {
        guard("listing prompts", self.0.list_prompts(request, context)).await
    }

    async fn list_resources(
        &self,
        request: Option<PaginatedRequestParams>,
        context: RequestContext<RoleServer>,
    ) -> Result<ListResourcesResult, ErrorData> {
        guard("listing resources", self.0.list_resources(request, context)).await
    }

    async fn list_resource_templates(
        &self,
        request: Option<PaginatedRequestParams>,
        context: RequestContext<RoleServer>,
    ) -> Result<ListResourceTemplatesResult, ErrorData> {
        guard(
            "listing resource templates",
            self.0.list_resource_templates(request, context),
        )
        .await
    }

    async fn read_resource(
        &self,
        request: ReadResourceRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<ReadResourceResponse, ErrorData> {
        guard("reading a resource", self.0.read_resource(request, context)).await
    }

    async fn listen(&self, context: SubscriptionContext) -> Result<(), ErrorData> {
        guard("listening for updates", self.0.listen(context)).await
    }

    #[expect(
        deprecated,
        reason = "the trait still has it, so the wrapper still forwards it"
    )]
    async fn subscribe(
        &self,
        request: SubscribeRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<(), ErrorData> {
        guard("subscribing", self.0.subscribe(request, context)).await
    }

    #[expect(
        deprecated,
        reason = "the trait still has it, so the wrapper still forwards it"
    )]
    async fn unsubscribe(
        &self,
        request: UnsubscribeRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<(), ErrorData> {
        guard("unsubscribing", self.0.unsubscribe(request, context)).await
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

    async fn on_custom_request(
        &self,
        request: CustomRequest,
        context: RequestContext<RoleServer>,
    ) -> Result<CustomResult, ErrorData> {
        guard(
            "answering a request",
            self.0.on_custom_request(request, context),
        )
        .await
    }

    async fn get_task(
        &self,
        request: GetTaskParams,
        context: RequestContext<RoleServer>,
    ) -> Result<GetTaskResult, ErrorData> {
        guard("reading a task", self.0.get_task(request, context)).await
    }

    async fn update_task(
        &self,
        request: UpdateTaskParams,
        context: RequestContext<RoleServer>,
    ) -> Result<(), ErrorData> {
        guard("updating a task", self.0.update_task(request, context)).await
    }

    async fn cancel_task(
        &self,
        request: CancelTaskParams,
        context: RequestContext<RoleServer>,
    ) -> Result<(), ErrorData> {
        guard("cancelling a task", self.0.cancel_task(request, context)).await
    }

    // -----------------------------------------------------------------------
    // Notifications. No id, no response, so there is nowhere to put an error
    // and nothing waiting on one — the guard's whole argument is that a panic
    // should become the answer, and here there is no answer. A panic in one
    // of these unwinds the task that delivered the notification and stops
    // there; #26 is where it reaches a log.
    // -----------------------------------------------------------------------

    async fn on_cancelled(
        &self,
        notification: CancelledNotificationParam,
        context: NotificationContext<RoleServer>,
    ) {
        self.0.on_cancelled(notification, context).await
    }

    async fn on_progress(
        &self,
        notification: ProgressNotificationParam,
        context: NotificationContext<RoleServer>,
    ) {
        self.0.on_progress(notification, context).await
    }

    async fn on_initialized(&self, context: NotificationContext<RoleServer>) {
        self.0.on_initialized(context).await
    }

    async fn on_roots_list_changed(&self, context: NotificationContext<RoleServer>) {
        self.0.on_roots_list_changed(context).await
    }

    async fn on_custom_notification(
        &self,
        notification: CustomNotification,
        context: NotificationContext<RoleServer>,
    ) {
        self.0.on_custom_notification(notification, context).await
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
