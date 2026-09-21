//! `list_package_versions`, driven the way an agent drives it.
//!
//! The same two seams as `tests/list_package_files.rs`: most of what is here
//! goes over the wire, because a test that only called the handler would keep
//! passing while the definition beside it stopped matching. The handler is
//! reached directly only where the question is about the answer's *type*.
//!
//! The documents below are `fixtures/versions/`, keyed by the URL
//! `src/registry.rs` builds, so a fetch that built a URL of its own finds
//! nothing.

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

const TOOL: &str = "list_package_versions";

/// The version documents this suite is served from, instead of the registries.
const FIXTURES: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/fixtures/versions");

// ---------------------------------------------------------------------------
// What it answers
// ---------------------------------------------------------------------------

/// The tracer bullet: one npm package, newest release first.
///
/// `zod`'s fixture is built so that the right answer is not reachable by
/// accident. Its newest release is `1.0.2` — a patch to the 1.x line
/// published after 2.0.0 was, which is what npm's own `@types/node` does
/// every week. So the expected order below is not the semver order, not the
/// lexical order of the keys, and not the order the document is written in.
/// A listing that took any of those three would have to disagree with it.
#[tokio::test]
async fn an_npm_packages_versions_come_back_newest_first() {
    let result = call(json!({
        "registry": "npm",
        "package": "zod",
    }))
    .await;

    assert_eq!(
        result["isError"],
        json!(false),
        "a package the fixture set has is not an error, got {result}"
    );

    assert_eq!(
        versions(&result),
        vec!["1.0.2", "1.0.10", "2.0.0", "1.0.0"],
        "newest first means most recently published first: 1.0.2 is a patch \
         to the 1.x line published after 2.0.0, so a listing sorted by \
         version number or by the document's own order gets this wrong: got \
         {result}"
    );
}

// ---------------------------------------------------------------------------
// Reading the answer
// ---------------------------------------------------------------------------

/// The `version` of every entry on this page, in the order they were returned.
fn versions(result: &Value) -> Vec<&str> {
    result["structuredContent"]["items"]
        .as_array()
        .unwrap_or_else(|| panic!("a page carries its items, got {result}"))
        .iter()
        .map(|entry| {
            entry["version"]
                .as_str()
                .unwrap_or_else(|| panic!("every entry names a version, got {entry}"))
        })
        .collect()
}

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
/// version documents come from `fixtures/versions/` rather than from the
/// registries.
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
