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

use crate::entries::Entries;
use crate::mcp::{self, Diffpack};
use crate::tools::Ctx;

pub const ALLOWED_ORIGINS: &str = "DIFFPACK_ALLOWED_ORIGINS";

pub fn router() -> Router {
    router_with(|| Ok(Diffpack::new()), allowed_origins_from_env())
        .merge(Entries::over(Ctx::new).routes())
}

pub fn router_with<S, F>(service_factory: F, allowed_origins: Vec<String>) -> Router
where
    F: Fn() -> Result<S, std::io::Error> + Send + Sync + 'static,
    S: ServerHandler + Send + 'static,
{
    let mcp = StreamableHttpService::new(
        move || service_factory().map(mcp::Guarded),
        Arc::new(NeverSessionManager::default()),
        mcp::transport_config(allowed_origins),
    );

    Router::new()
        .route("/health", get(health))
        .nest_service("/mcp", CatchPanicLayer::custom(JsonRpcOnPanic).layer(mcp))
        .fallback(not_found)
        .layer(axum::middleware::map_response(unbuffered_streams))
}

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

const INTERNAL_ERROR: i32 = -32603;

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
