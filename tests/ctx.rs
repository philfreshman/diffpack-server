//! The context a test builds.
//!
//! Every other suite here builds a `Ctx` to drive one tool with, and reads
//! that tool's answer. What none of them reads is the context itself: which
//! adapter each of its seams got. A seam a suite does not use is a seam
//! nothing asserts anything about, and for as long as its tool does not exist
//! that is invisible — until the tool arrives, reaches the seam through a
//! context built for a different one, and asks a registry from CI.
//!
//! So this suite drives the seams a context carries rather than a tool: one
//! tool per seam, chosen because it is the shortest path to that seam and not
//! because this is a suite about it. Both go over the wire, through the
//! service factory `router_with` takes, because building a context is only
//! interesting if it is the context a handler is actually handed.

use axum::body::Body;
use axum::http::Request;
use diffpack_server::mcp::Diffpack;
use diffpack_server::router;
use diffpack_server::tools::Ctx;
use http_body_util::BodyExt;
use serde_json::{json, Value};
use tower::ServiceExt;

const CURRENT: &str = "2026-07-28";

/// The fixture sets, as every suite is served from them.
const FIXTURES: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/fixtures");

/// A root with nothing under it.
///
/// The fixture adapters answer a directory they cannot read with
/// [`diffpack_server::error::Failure::Internal`], which is the refusal this
/// suite is after: a live adapter has no way to produce it, so a seam that
/// answers with it is a seam that never left this machine.
const NO_FIXTURES: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/fixtures/no-set-is-checked-in-here"
);

/// Every seam is the fixture set's, and the right part of it.
///
/// One call per seam, each asserting something the registry it stands in for
/// could not answer with: the fixture `serde` carries a `src/lib.rs` of one
/// line that the real crate does not, and the fixture `zod` has four versions
/// where the real package has hundreds. So this fails if a seam was wired to
/// another seam's directory, and it fails if a seam was left live and the
/// machine happened to have a network.
#[tokio::test]
async fn every_seam_a_context_carries_is_the_fixture_set() {
    let file = call(
        FIXTURES,
        "get_file_content",
        json!({
            "registry": "crates",
            "package": "serde",
            "version": "1.0.0",
            "path": "src/lib.rs",
        }),
    )
    .await;

    assert_eq!(
        file["result"]["structuredContent"]["text"], "pub fn serialize() {}\n",
        "the archive seam should read `fixtures/archives/`, got {file}"
    );

    let versions = call(
        FIXTURES,
        "list_package_versions",
        json!({ "registry": "npm", "package": "zod" }),
    )
    .await;

    assert_eq!(
        versions["result"]["structuredContent"]["total"],
        json!(4),
        "the catalogue seam should read `fixtures/versions/`, got {versions}"
    );
}

/// And none of them can reach a registry.
///
/// A context over a root that holds nothing: every seam refuses with the
/// failure its fixture adapter produces when it cannot read its set, which is
/// the internal channel and `-32000`. A seam the constructor had left live
/// would answer here instead, or fail naming somebody else's server.
///
/// This is what the two builders it replaced could not promise. Each filled
/// the seam it was not given from the live constructor, so `Ctx` over a
/// fixture archive carried a live catalogue: the first tool to read a
/// catalogue through one would have made a real request, in CI,
/// intermittently, and reported it as the registry being unreachable.
#[tokio::test]
async fn no_seam_a_context_carries_can_reach_a_registry() {
    for (tool, arguments, doing) in [
        (
            "get_file_content",
            json!({
                "registry": "crates",
                "package": "serde",
                "version": "1.0.0",
                "path": "src/lib.rs",
            }),
            "reading the archive fixtures",
        ),
        (
            "list_package_versions",
            json!({ "registry": "npm", "package": "zod" }),
            "reading the version fixtures",
        ),
    ] {
        let answer = call(NO_FIXTURES, tool, arguments).await;

        assert_eq!(
            answer["error"]["code"], -32000,
            "`{tool}` should have failed reading a fixture set that is not \
             there, got {answer}"
        );
        assert_eq!(
            answer["error"]["message"],
            format!("diffpack failed while {doing}."),
            "`{tool}` should have gone to the fixture adapter, got {answer}"
        );
    }
}

/// `tools/call` for `tool`, against a server whose context is built over
/// `fixtures`.
///
/// Answers the whole envelope rather than the result, because half of what is
/// asserted above is a JSON-RPC error and the other half is a result.
async fn call(fixtures: &'static str, tool: &str, arguments: Value) -> Value {
    let body = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "tools/call",
        "params": {
            "name": tool,
            "arguments": arguments,
            "_meta": {
                "io.modelcontextprotocol/protocolVersion": CURRENT,
                "io.modelcontextprotocol/clientCapabilities": {},
            },
        },
    });

    let request = Request::builder()
        .method("POST")
        .uri("/mcp")
        .header("host", "mcp.diffpack.io")
        .header("accept", "application/json, text/event-stream")
        .header("content-type", "application/json")
        .header("mcp-protocol-version", CURRENT)
        .header("mcp-method", "tools/call")
        .header("mcp-name", tool)
        .body(Body::from(body.to_string()))
        .expect("the request should build");

    let router = router::router_with(
        move || Ok(Diffpack::with_ctx(Ctx::fixture(fixtures))),
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
