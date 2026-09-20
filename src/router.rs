//! Every route this function serves, in one place.
//!
//! `vercel.json` rewrites `/(.*)` to the single handler, so routing is this
//! crate's job rather than the platform's. `api/mcp.rs` does nothing but wrap
//! [`router`] in `vercel_runtime`'s layer and hand it to the runtime, which
//! means the tests drive the same router production does — the socket is the
//! only thing they leave out.

use std::sync::Arc;

use axum::body::Body;
use axum::http::{header, HeaderValue, StatusCode};
use axum::response::Response;
use axum::routing::get;
use axum::{Json, Router};
use rmcp::transport::streamable_http_server::session::never::NeverSessionManager;
use rmcp::transport::streamable_http_server::StreamableHttpService;
use rmcp::ServerHandler;
use tower::Layer;
use tower_http::catch_panic::{CatchPanicLayer, ResponseForPanic};

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
        // Every handler the factory builds is wrapped, including one a test
        // supplies, because a panic guard that only covered the handler in
        // this file would be a guard the tests could not reach.
        move || service_factory().map(mcp::Guarded),
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
        // The panic layer wraps the MCP service rather than the whole router,
        // because what it answers with is JSON-RPC and only this route speaks
        // it. The other two routes are a constant and a literal; there is
        // nothing in them to panic.
        .nest_service("/mcp", CatchPanicLayer::custom(JsonRpcOnPanic).layer(mcp))
        .fallback(not_found)
        .layer(axum::middleware::map_response(unbuffered_streams))
}

/// Turn a panic in the transport into an answer.
///
/// # What this does and does not catch
///
/// This catches the part of a request that runs inline on the HTTP task:
/// origin validation, header checks, and deserializing the body — everything
/// `rmcp` does before it hands the message to a handler. That is the code
/// that meets attacker-controlled bytes first, which is why the layer is
/// worth having on a public endpoint.
///
/// It does **not** catch a panic in a handler. `rmcp` runs handlers on a task
/// of its own, so that unwind never passes through this stack;
/// [`crate::mcp::Guarded`] is what catches those, and it has to live inside
/// the handler to do it. The two are not alternatives — they cover different
/// halves of the request, and neither covers the other's.
///
/// The status is `200`, matching how the transport already carries an
/// internal error: the failure is in the body, not in the HTTP exchange,
/// which completed. The id is `null` because a panic here happened before or
/// during parsing, so there may be no id to echo — JSON-RPC allows null for
/// exactly that case.
///
/// The panic's own message is not included. It is written for us, it may name
/// a file in this repository, and the caller can do nothing with it; #26 is
/// where it reaches a log instead.
#[derive(Debug, Clone)]
struct JsonRpcOnPanic;

impl ResponseForPanic for JsonRpcOnPanic {
    type ResponseBody = Body;

    fn response_for_panic(
        &mut self,
        _panic: Box<dyn std::any::Any + Send + 'static>,
    ) -> axum::http::Response<Self::ResponseBody> {
        let body = serde_json::json!({
            "jsonrpc": "2.0",
            "id": null,
            "error": {
                "code": INTERNAL_ERROR,
                "message": "diffpack failed while handling this request.",
            },
        });

        axum::http::Response::builder()
            .status(StatusCode::OK)
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(body.to_string()))
            .expect("a constant response builds")
    }
}

/// JSON-RPC's own code for "something broke inside the server", which is what
/// a panic is. Not one of ours from [`crate::error`]: those name a failure we
/// anticipated, and a panic is by definition one we did not.
const INTERNAL_ERROR: i32 = -32603;

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
