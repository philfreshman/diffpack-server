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
//! call per seam, chosen because it is the shortest path to that seam and not
//! because this is a suite about the tool making it. Each goes over the wire,
//! through the service factory `router_with` takes, because building a
//! context is only interesting if it is the context a handler is handed.
//!
//! Which seams those are is `Ctx::seams`'s answer and not this file's list,
//! so a seam added to a context with no call written for it fails here rather
//! than waiting for the day it is live.

use std::path::Path;

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

/// One seam a context carries, and the shortest call that reaches it.
struct Seam {
    /// What [`Ctx::seams`] calls it.
    name: &'static str,

    tool: &'static str,
    arguments: Value,

    /// Where in that tool's answer to read, and what the fixture set says
    /// there — a fact the registry this seam stands in for could not have
    /// answered with.
    reads: &'static str,
    fixture_says: Value,

    /// What the fixture adapter says it was doing when it could not find its
    /// set, which is how a refusal is known to have come from disk.
    doing: &'static str,
}

/// Every seam, in the order a context carries them.
///
/// Adding one here is the second half of adding one to `Ctx`: the first half
/// is the compile error in both of its constructors.
fn seams() -> Vec<Seam> {
    vec![
        Seam {
            name: "archive",
            tool: "get_file_content",
            arguments: json!({
                "registry": "crates",
                "package": "serde",
                "version": "1.0.0",
                "path": "src/lib.rs",
            }),
            reads: "/text",
            // The fixture `serde` carries a `src/lib.rs` of one line that the
            // real crate does not.
            fixture_says: json!("pub fn serialize() {}\n"),
            doing: "reading the archive fixtures",
        },
        Seam {
            name: "catalogue",
            tool: "list_package_versions",
            arguments: json!({ "registry": "npm", "package": "zod" }),
            reads: "/total",
            // The fixture `zod` has four versions where the real package has
            // hundreds.
            fixture_says: json!(4),
            doing: "reading the version fixtures",
        },
    ]
}

/// Every seam a context carries has a call here.
///
/// The list above is held to `Ctx::seams`, which is the struct's own answer,
/// so the next seam cannot arrive without one. Without this the two tests
/// below would keep passing while saying nothing about it, which is the shape
/// of the bug this suite exists for one level up.
#[test]
fn every_seam_a_context_carries_is_driven_here() {
    let mut driven: Vec<&str> = seams().iter().map(|seam| seam.name).collect();
    driven.sort_unstable();

    let mut carried = Ctx::fixture(FIXTURES).seams().to_vec();
    carried.sort_unstable();

    assert_eq!(
        driven, carried,
        "a seam with no call here is a seam nothing holds to the fixture \
         set: give it the shortest call that reaches it"
    );
}

/// Every seam is the fixture set's, and the right part of it.
///
/// Each call asserts something the registry it stands in for could not answer
/// with. So this fails if a seam was wired to another seam's directory, and
/// it fails if a seam was left live and the machine happened to have a
/// network.
#[tokio::test]
async fn every_seam_a_context_carries_is_the_fixture_set() {
    for seam in seams() {
        let answer = call(FIXTURES, seam.tool, seam.arguments).await;
        let read = answer["result"]["structuredContent"].pointer(seam.reads);

        assert_eq!(
            read,
            Some(&seam.fixture_says),
            "the `{}` seam should have read the fixture set, got {answer}",
            seam.name
        );
    }
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
    assert!(
        !Path::new(NO_FIXTURES).exists(),
        "`{NO_FIXTURES}` has to hold nothing for this test to mean anything, \
         and something is checked in there"
    );

    for seam in seams() {
        let answer = call(NO_FIXTURES, seam.tool, seam.arguments).await;

        assert_eq!(
            answer["error"]["code"], -32000,
            "the `{}` seam should have failed reading a fixture set that is \
             not there, got {answer}",
            seam.name
        );
        assert_eq!(
            answer["error"]["message"],
            format!("diffpack failed while {}.", seam.doing),
            "the `{}` seam should have gone to the fixture adapter, got \
             {answer}",
            seam.name
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
