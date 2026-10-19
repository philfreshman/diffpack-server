#![allow(dead_code)]

use std::sync::Arc;

use axum::body::Body;
use axum::http::{HeaderMap, Request, StatusCode};
use axum::Router;
use diffpack_server::mcp::Diffpack;
use diffpack_server::router;
use diffpack_server::tools::Ctx;
use http_body_util::BodyExt;
use serde_json::{json, Value};
use tower::ServiceExt;

pub const CURRENT: &str = "2026-07-28";

pub const PREVIOUS: &str = "2025-11-25";

pub const FIXTURES: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/fixtures");

type Build = Arc<dyn Fn() -> Router + Send + Sync>;

#[derive(Clone)]
pub struct Client {
    build: Build,
    speaks: &'static str,
}

impl Client {
    pub fn fixture() -> Self {
        Self::serving(|| Ctx::fixture(FIXTURES))
    }

    pub fn over(ctx: Ctx) -> Self {
        Self::serving(move || ctx.clone())
    }

    fn serving(build: impl Fn() -> Ctx + Send + Sync + 'static) -> Self {
        let build = Arc::new(build);
        Self::routed(move || {
            let build = Arc::clone(&build);
            router::router_with(move || Ok(Diffpack::with_ctx(build())), Vec::new())
        })
    }

    pub fn routed(build: impl Fn() -> Router + Send + Sync + 'static) -> Self {
        Self {
            build: Arc::new(build),
            speaks: CURRENT,
        }
    }

    pub fn speaking(self, version: &'static str) -> Self {
        Self {
            speaks: version,
            ..self
        }
    }

    pub async fn call(&self, tool: &str, arguments: Value) -> Value {
        self.respond(json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "tools/call",
            "params": { "name": tool, "arguments": arguments },
        }))
        .await
        .result()
    }

    pub async fn tools(&self) -> Vec<Value> {
        let answer = self
            .post(json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "tools/list",
                "params": {},
            }))
            .await;

        answer["result"]["tools"]
            .as_array()
            .unwrap_or_else(|| panic!("tools/list should answer with an array, got {answer}"))
            .clone()
    }

    pub async fn listed(&self, name: &str) -> Value {
        let tools = self.tools().await;

        tools
            .iter()
            .find(|tool| tool["name"] == name)
            .unwrap_or_else(|| {
                let names: Vec<&str> = tools.iter().filter_map(|t| t["name"].as_str()).collect();
                panic!("`{name}` should be listed, got {names:?}")
            })
            .clone()
    }

    pub async fn post(&self, body: Value) -> Value {
        self.respond(body).await.json()
    }

    pub async fn respond(&self, body: Value) -> Answer {
        self.send(self.request(body)).await
    }

    pub async fn send(&self, request: Request<Body>) -> Answer {
        let response = (self.build)()
            .oneshot(request)
            .await
            .expect("the router answers every request");

        let status = response.status();
        let headers = response.headers().clone();
        let body = response
            .into_body()
            .collect()
            .await
            .expect("the body should read")
            .to_bytes()
            .to_vec();

        Answer {
            status,
            headers,
            body,
        }
    }

    fn envelope(&self, body: Value) -> Value {
        let mut body = body;
        if self.speaks >= CURRENT {
            body["params"]["_meta"] = json!({
                "io.modelcontextprotocol/protocolVersion": self.speaks,
                "io.modelcontextprotocol/clientCapabilities": {},
            });
        }
        body
    }

    pub fn request(&self, body: Value) -> Request<Body> {
        let body = self.envelope(body);

        let mut request = Request::builder()
            .method("POST")
            .uri("/mcp")
            .header("host", "mcp.diffpack.io")
            .header("accept", "application/json, text/event-stream")
            .header("content-type", "application/json")
            .header("mcp-protocol-version", self.speaks);

        if self.speaks >= CURRENT {
            let method = body["method"].as_str().expect("a call names a method");
            request = request.header("mcp-method", method);

            for named in ["name", "uri"] {
                if let Some(names) = body["params"][named].as_str() {
                    request = request.header("mcp-name", names);
                }
            }
        }

        request
            .body(Body::from(body.to_string()))
            .expect("the request should build")
    }
}

pub struct Answer {
    pub status: StatusCode,
    headers: HeaderMap,
    body: Vec<u8>,
}

impl Answer {
    pub fn json(&self) -> Value {
        serde_json::from_slice(&self.body).unwrap_or_else(|e| {
            panic!(
                "expected a JSON body ({}), got {e}: {}",
                self.status,
                String::from_utf8_lossy(&self.body)
            )
        })
    }

    pub fn result(&self) -> Value {
        let body = self.json();
        if let Some(error) = body.get("error") {
            panic!("expected a result, got JSON-RPC error {error}");
        }
        body["result"].clone()
    }

    pub fn frame(&self) -> String {
        String::from_utf8_lossy(&self.body).into_owned()
    }

    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers.get(name).and_then(|value| value.to_str().ok())
    }
}

pub fn names(text: &str, word: &str) -> bool {
    text.split(|c: char| !c.is_alphanumeric() && c != '.')
        .any(|candidate| candidate.trim_end_matches('.') == word)
}
