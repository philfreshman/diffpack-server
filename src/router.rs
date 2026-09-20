//! Every route this function serves, in one place.
//!
//! `vercel.json` rewrites `/(.*)` to the single handler, so routing is this
//! crate's job rather than the platform's. `api/mcp.rs` does nothing but wrap
//! [`router`] in `vercel_runtime`'s layer and hand it to the runtime, which
//! means the tests drive the same router production does — the socket is the
//! only thing they leave out.

use std::sync::Arc;

use axum::http::{header, HeaderValue, StatusCode};
use axum::response::Response;
use axum::routing::get;
use axum::{Json, Router};
use rmcp::transport::streamable_http_server::session::never::NeverSessionManager;
use rmcp::transport::streamable_http_server::StreamableHttpService;
use rmcp::ServerHandler;

use crate::mcp::{self, Diffpack};

/// The environment variable holding the browser origins allowed to drive the
/// MCP endpoint, comma separated.
///
/// Configuration rather than a constant because the answer differs per
/// deployment and because adding a browser-based client should not need a
/// release. Unset means the closed default: no browser origin is allowed, and
/// the local clients — which send no `Origin` at all — are unaffected.
pub const ALLOWED_ORIGINS: &str = "DIFFPACK_ALLOWED_ORIGINS";

/// The router `api/mcp.rs` serves.
pub fn router() -> Router {
    router_with(|| Ok(Diffpack::new()), allowed_origins_from_env())
}

/// The router, with the MCP handler and the allowed origins supplied.
///
/// `service_factory` runs **per request**, which is why it is a closure and
/// not a value: shared state — the HTTP client, the Blob client of #20 — is
/// captured by it and cloned into each handler, rather than being rebuilt on
/// every call or stored somewhere that has to outlive an invocation.
///
/// Taking the factory as a parameter is also the seam the tests need: an
/// origin list they chose, and (in #7) a handler that panics on purpose.
pub fn router_with<S, F>(service_factory: F, allowed_origins: Vec<String>) -> Router
where
    F: Fn() -> Result<S, std::io::Error> + Send + Sync + 'static,
    S: ServerHandler + Send + 'static,
{
    let mcp = StreamableHttpService::new(
        service_factory,
        // Not the local session manager with sessions switched off, but the
        // manager that cannot make one. Two invocations of a serverless
        // function share no memory, so an in-memory session store would be a
        // map that is always empty by the time it is read; saying so in the
        // type means the answer to "where did the session go" is that there
        // was never one to go.
        Arc::new(NeverSessionManager::default()),
        mcp::transport_config(allowed_origins),
    );

    Router::new()
        .route("/health", get(health))
        .nest_service("/mcp", mcp)
        .fallback(not_found)
        .layer(axum::middleware::map_response(unbuffered_streams))
}

/// The allowed origins named by [`ALLOWED_ORIGINS`], or none.
///
/// Empty entries are dropped so that a trailing comma, or the variable set to
/// the empty string, means "none" rather than an origin named "".
pub fn allowed_origins_from_env() -> Vec<String> {
    std::env::var(ALLOWED_ORIGINS)
        .unwrap_or_default()
        .split(',')
        .map(str::trim)
        .filter(|origin| !origin.is_empty())
        .map(str::to_owned)
        .collect()
}

async fn health() -> Json<serde_json::Value> {
    Json(crate::health::body())
}

async fn not_found() -> (StatusCode, Json<serde_json::Value>) {
    (
        StatusCode::NOT_FOUND,
        Json(serde_json::json!({ "error": "not found" })),
    )
}

/// Tell any proxy in front of us not to buffer an event stream.
///
/// Buffering a stream defeats the only reason to send one: the client waits
/// for the whole body anyway, and on a long diff it waits past a timeout
/// instead of seeing progress. `X-Accel-Buffering: no` is the header nginx
/// and the CDNs that copied it read. It is set only on `text/event-stream`,
/// because on an ordinary JSON answer buffering is what we want.
async fn unbuffered_streams(mut response: Response) -> Response {
    let is_event_stream = response
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|content_type| content_type.starts_with("text/event-stream"));

    if is_event_stream {
        response
            .headers_mut()
            .insert("x-accel-buffering", HeaderValue::from_static("no"));
    }

    response
}
