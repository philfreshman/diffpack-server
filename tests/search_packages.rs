//! `search_packages`, driven the way an agent drives it.
//!
//! The same two seams the other tool suites use, for the same reason. Most of
//! what is here goes over the wire — `tools/list` for what a client is told,
//! `tools/call` for what it gets back — because a test that only called the
//! handler would keep passing while the definition beside it stopped
//! matching. The handler is reached directly only where the question is about
//! the answer's *type* or about which `Failure` a path produces.
//!
//! The bodies under `fixtures/searches/` are the three sources' own answers,
//! cut down to the fields a hit carries: npm's `objects`, crates.io's
//! `crates`, and a few lines of PyPI's index. The set is keyed by URL, so a
//! search that built a URL of its own finds nothing there — which makes this
//! a test of where each registry is asked as well as of what comes back.
//!
//! What is deliberately not re-proven here: how a query is matched and
//! ordered against PyPI's index. That is `crate::registry`'s and
//! `tests/registry.rs` holds it against a body it can state in full.

use axum::body::Body;
use axum::http::Request;
use diffpack_server::catalogue::Catalogue;
use diffpack_server::mcp::Diffpack;
use diffpack_server::router;
use diffpack_server::tools::Ctx;
use http_body_util::BodyExt;
use serde_json::{json, Value};
use tower::ServiceExt;

const CURRENT: &str = "2026-07-28";

const TOOL: &str = "search_packages";

/// The search answers this suite is served, instead of the registries'.
const FIXTURES: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/fixtures/searches");

// ---------------------------------------------------------------------------
// What it answers
// ---------------------------------------------------------------------------

/// The whole of what this tool is for: a name half-remembered, and the
/// packages a registry has that answer to it. npm carries all three fields a
/// hit can have, so this is also where a full hit is asserted.
#[tokio::test]
async fn an_npm_search_answers_with_the_packages_it_found() {
    let result = call(json!({
        "registry": "npm",
        "query": "zod",
    }))
    .await;

    let items = &result["structuredContent"]["items"];
    assert_eq!(
        items[0],
        json!({
            "name": "zod",
            "version": "4.0.0",
            "description": "TypeScript-first schema declaration and validation library \
                            with static type inference",
        }),
        "the first hit is the package itself, with everything npm says about it, got {items}"
    );
    assert_eq!(
        items[1]["name"], "zod-to-json-schema",
        "and the rest follow in the order npm ranked them, got {items}"
    );
    assert_eq!(
        result["isError"],
        json!(false),
        "packages found is not an error, got {result}"
    );
}

// ---------------------------------------------------------------------------
// Driving the endpoint
// ---------------------------------------------------------------------------

/// Call the tool with `arguments`, returning the `result` — or panicking with
/// the JSON-RPC error, so a failure says what the server objected to.
async fn call(arguments: Value) -> Value {
    let answer = post(json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "tools/call",
        "params": { "name": TOOL, "arguments": arguments, "_meta": meta() },
    }))
    .await;

    if let Some(error) = answer.get("error") {
        panic!("expected a result, got JSON-RPC error {error}");
    }
    answer["result"].clone()
}

/// The per-request `_meta` a `2026-07-28` client attaches. See `tests/mcp.rs`.
fn meta() -> Value {
    json!({
        "io.modelcontextprotocol/protocolVersion": CURRENT,
        "io.modelcontextprotocol/clientCapabilities": {},
    })
}

/// A request as a conforming `2026-07-28` client sends it, to a server whose
/// searches are answered from `fixtures/searches/` rather than by the
/// registries.
///
/// The fixture adapter is reached through the service factory `router_with`
/// takes, which is the path production takes to build the `Ctx` every handler
/// is handed.
async fn post(body: Value) -> Value {
    let method = body["method"].as_str().expect("a call names a method");

    let mut request = Request::builder()
        .method("POST")
        .uri("/mcp")
        .header("host", "mcp.diffpack.io")
        .header("accept", "application/json, text/event-stream")
        .header("content-type", "application/json")
        .header("mcp-protocol-version", CURRENT)
        .header("mcp-method", method);

    if let Some(name) = body["params"]["name"].as_str() {
        request = request.header("mcp-name", name);
    }

    let request = request
        .body(Body::from(body.to_string()))
        .expect("the request should build");

    let router = router::router_with(
        || {
            Ok(Diffpack::with_ctx(Ctx::with_catalogue(Catalogue::fixture(
                FIXTURES,
            ))))
        },
        Vec::new(),
    );

    let response = router
        .oneshot(request)
        .await
        .expect("the router answers every request");

    let status = response.status();
    let bytes = response
        .into_body()
        .collect()
        .await
        .expect("the body should read")
        .to_bytes();

    serde_json::from_slice(&bytes).unwrap_or_else(|e| {
        panic!(
            "expected a JSON body ({status}), got {e}: {}",
            String::from_utf8_lossy(&bytes)
        )
    })
}
