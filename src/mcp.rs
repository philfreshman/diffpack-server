use std::borrow::Cow;
use std::future::Future;
use std::panic::AssertUnwindSafe;

use futures::FutureExt as _;
#[expect(
    deprecated,
    reason = "forwarding a deprecated method is still forwarding"
)]
use rmcp::model::SetLevelRequestParams;
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
use rmcp::service::{NotificationContext, RequestContext, SubscriptionContext};
use rmcp::transport::streamable_http_server::StreamableHttpServerConfig;
use rmcp::{ErrorData, RoleServer, ServerHandler};

use crate::error::Failure;
use crate::log::Line;
use crate::prompts;
use crate::resources::{self, Address};
use crate::tools::{self, Ctx};

pub const NAME: &str = "diffpack";

pub const TOOL_LIST_TTL_MS: u64 = 60 * 60 * 1000;

#[derive(Debug, Clone, Default)]
pub struct Diffpack {
    ctx: Ctx,
}

impl Diffpack {
    pub fn new() -> Self {
        Self::with_ctx(Ctx::new())
    }

    pub fn with_ctx(ctx: Ctx) -> Self {
        Self { ctx }
    }
}

impl ServerHandler for Diffpack {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(
            ServerCapabilities::builder()
                .enable_tools()
                .enable_resources()
                .enable_prompts()
                .build(),
        )
        .with_server_info(Implementation::new(NAME, env!("CARGO_PKG_VERSION")))
        .with_instructions(
            "Diff two versions of a published package and read the result. \
             Works with npm, crates.io, PyPI and Go. Start with `tools/list`: \
             every tool takes a registry, a package name and a version, and \
             names the registry the way that registry does. `resources/list` \
             has the registries described in one document, and a comparison \
             you have already made can be read back by URI as well as by \
             tool. `prompts/list` has a prompt that walks through reviewing \
             an upgrade from one version to another.",
        )
    }

    fn supported_protocol_versions(&self) -> Cow<'static, [ProtocolVersion]> {
        Cow::Borrowed(ProtocolVersion::KNOWN_VERSIONS)
    }

    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, ErrorData> {
        Ok(ListToolsResult::with_all_items(tools::definitions())
            .with_ttl_ms(TOOL_LIST_TTL_MS)
            .with_cache_scope(CacheScope::Public))
    }

    fn get_tool(&self, name: &str) -> Option<Tool> {
        tools::definition_of(name)
    }

    async fn list_resources(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListResourcesResult, ErrorData> {
        Ok(ListResourcesResult::with_all_items(resources::catalogue())
            .with_ttl_ms(TOOL_LIST_TTL_MS)
            .with_cache_scope(CacheScope::Public))
    }

    async fn list_resource_templates(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListResourceTemplatesResult, ErrorData> {
        Ok(
            ListResourceTemplatesResult::with_all_items(resources::templates())
                .with_ttl_ms(TOOL_LIST_TTL_MS)
                .with_cache_scope(CacheScope::Public),
        )
    }

    async fn read_resource(
        &self,
        request: ReadResourceRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<ReadResourceResponse, ErrorData> {
        let address = Address::of(&request.uri);
        let line = Line::reading(address.name_in_line()).about(Some(&address.arguments()));

        tools::run(&self.ctx, line, async |call| address.read(call).await)
            .await
            .map(ReadResourceResponse::from)
            .map_err(Failure::refuse)
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, ErrorData> {
        tools::call(&request.name, request.arguments, &self.ctx)
            .await
            .map(CallToolResponse::from)
    }

    async fn list_prompts(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListPromptsResult, ErrorData> {
        Ok(ListPromptsResult::with_all_items(prompts::definitions())
            .with_ttl_ms(TOOL_LIST_TTL_MS)
            .with_cache_scope(CacheScope::Public))
    }

    async fn get_prompt(
        &self,
        request: GetPromptRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<GetPromptResponse, ErrorData> {
        prompts::get(&request.name, request.arguments)
            .map(GetPromptResponse::from)
            .map_err(Failure::refuse)
    }
}

#[derive(Debug, Clone)]
pub struct Guarded<S>(pub S);

impl<S: ServerHandler> ServerHandler for Guarded<S> {
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

pub fn transport_config(allowed_origins: Vec<String>) -> StreamableHttpServerConfig {
    StreamableHttpServerConfig::default()
        .with_legacy_session_mode(false)
        .with_json_response(true)
        .disable_allowed_hosts()
        .enforce_origin_validation()
        .with_allowed_origins(allowed_origins)
}
