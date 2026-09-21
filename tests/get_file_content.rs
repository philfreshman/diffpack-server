//! `get_file_content`, driven the way an agent drives it.
//!
//! Two seams, the same two `tests/list_package_files.rs` uses and for the
//! same reason. Most of what is here goes over the wire — `tools/list` for
//! what a client is told, `tools/call` for what it gets back — because a test
//! that only called the handler would keep passing while the definition
//! beside it stopped matching, which is the failure #41 exists to prevent.
//! The handler is reached directly only where the question is about the
//! answer's *type* rather than about its JSON.
//!
//! What is deliberately not re-proven here: where a cut falls, what the
//! marker says, and that the ceiling is measured on serialised bytes rather
//! than on length. Those are `src/page.rs`'s and `tests/page.rs` holds them
//! against generated text, which is a stronger fixture than any file in a
//! package. What this suite asserts is that this tool goes *through* that
//! module rather than around it.
//!
//! Nor that no `description` an agent reads names a Rust path — that is a
//! rule every tool is held to rather than a fact about this one, so
//! `tests/tools.rs` holds it over the whole of `tools/list`.
//!
//! Extraction, and the lossy decoding a file that is not UTF-8 goes through,
//! are `tests/archive.rs`'s. The archives below are the fixture set, so a
//! fetch that built a URL of its own finds nothing.

use axum::body::Body;
use axum::http::Request;
use diffpack_server::archive::Archive;
use diffpack_server::mcp::Diffpack;
use diffpack_server::router;
use diffpack_server::tools::Ctx;
use http_body_util::BodyExt;
use serde_json::{json, Value};
use tower::ServiceExt;

const CURRENT: &str = "2026-07-28";

const TOOL: &str = "get_file_content";

/// The archives this suite is served from, instead of the registries.
const FIXTURES: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/fixtures/archives");

// ---------------------------------------------------------------------------
// What it answers
// ---------------------------------------------------------------------------

/// The tracer bullet: one file out of one npm package, exactly as it was
/// packed.
///
/// The expected text is the fixture's own, written by hand and packed by
/// `tar` when `scripts/make-archive-fixtures.sh` was written — not read back
/// out of this server, which would agree with a bug.
///
/// The path is `package.json` rather than `package/package.json`: the
/// archive's top-level directory is stripped before a path is ever asked
/// for, which is the one thing about a path an agent cannot infer.
#[tokio::test]
async fn a_file_from_an_npm_package_comes_back_with_its_exact_content() {
    let result = call(json!({
        "registry": "npm",
        "package": "@types/node",
        "version": "20.1.0",
        "path": "package.json",
    }))
    .await;

    assert_eq!(
        result["isError"],
        json!(false),
        "a file the fixture set has is not an error, got {result}"
    );
    assert_eq!(
        result["structuredContent"]["text"],
        "{\n  \"name\": \"@types/node\",\n  \"version\": \"20.1.0\"\n}\n",
        "got {result}"
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
/// archives come from `fixtures/archives/` rather than from the registries.
///
/// The fixture adapter is reached the way #39 says a tool's state is reached:
/// through the service factory `router_with` takes, which builds the [`Ctx`]
/// every handler is handed. A test that reached around it would be testing a
/// path production does not take.
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
            Ok(Diffpack::with_ctx(Ctx::with_archive(Archive::fixture(
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
