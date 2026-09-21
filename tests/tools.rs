//! A tool, driven the way an agent drives one.
//!
//! The seam under test is a tool module's interface, and it is deliberately
//! not reached directly: no test here calls `definition()` or a handler. Both
//! go over the wire — `tools/list` for what a client is told, `tools/call`
//! for what it gets back — because that is the only part a tool module is
//! promising anything about. A test that called the handler would keep
//! passing while the definition beside it stopped matching, which is the
//! failure #41 exists to prevent.
//!
//! `tests/mcp.rs` owns the transport and `tests/errors.rs` owns the two
//! channels. What is here is the shape of a tool: that its definition carries
//! everything an agent needs, and that its answer is structured, and that its
//! two kinds of failure land on the two different channels.

use axum::body::Body;
use axum::http::Request;
use diffpack_server::router;
use http_body_util::BodyExt;
use serde_json::{json, Value};
use tower::ServiceExt;

const CURRENT: &str = "2026-07-28";

/// The one tool this server has. #11 onward add the rest.
const TOOL: &str = "resolve_archive_url";

// ---------------------------------------------------------------------------
// What a client is told
// ---------------------------------------------------------------------------

/// Everything #23 asks a tool to carry, on the tool that exists. The test is
/// here rather than in #23 because the point of one module per tool is that
/// these come from the module rather than from a second list, so a tool that
/// arrives without them is a tool that never compiled.
#[tokio::test]
async fn a_tool_is_listed_with_everything_an_agent_needs() {
    let tool = listed(TOOL).await;

    assert!(
        tool["description"]
            .as_str()
            .is_some_and(|text| !text.is_empty()),
        "a tool an agent picks without documentation needs a description, got {tool}"
    );

    assert_eq!(
        tool["inputSchema"]["type"], "object",
        "the input schema should be an object schema, got {}",
        tool["inputSchema"]
    );
    for field in ["registry", "package", "version"] {
        assert!(
            tool["inputSchema"]["properties"][field].is_object(),
            "the input schema should describe `{field}`, got {}",
            tool["inputSchema"]
        );
    }

    assert_eq!(
        tool["outputSchema"]["type"], "object",
        "a tool that answers with structured content has to declare its shape, got {tool}"
    );

    assert_eq!(
        tool["annotations"]["readOnlyHint"], true,
        "resolving a URL changes nothing, and a client deciding whether to \
         ask for confirmation reads this, got {}",
        tool["annotations"]
    );
    assert_eq!(
        tool["annotations"]["idempotentHint"], true,
        "the same three arguments always resolve to the same URL, got {}",
        tool["annotations"]
    );
}

/// `registry` is the enum `src/registry.rs` defines, not a string a tool
/// describes in prose. The schema is where an agent learns what it may pass,
/// so a tool that spelled the list itself would be the fifth copy ADR 0004
/// rejects — and the one an agent reads first.
#[tokio::test]
async fn the_registry_parameter_is_the_enum_the_registry_module_owns() {
    let tool = listed(TOOL).await;
    let registry = &tool["inputSchema"]["properties"]["registry"];

    assert_eq!(
        registry["enum"],
        json!(["npm", "crates", "pypi"]),
        "the schema should list the registries this server has, got {registry}"
    );
}

// ---------------------------------------------------------------------------
// What it answers
// ---------------------------------------------------------------------------

/// npm serves a version's tarball at a path anyone can build, which is why
/// this tool can answer without asking npm anything. The expected URL is from
/// #10, not from running the code.
#[tokio::test]
async fn npm_resolves_to_the_registry_tarball() {
    let result = call(json!({
        "registry": "npm",
        "package": "zod",
        "version": "4.0.0",
    }))
    .await;

    assert_eq!(
        result["structuredContent"]["url"],
        "https://registry.npmjs.org/zod/-/zod-4.0.0.tgz"
    );
    assert_eq!(
        result["isError"],
        json!(false),
        "a resolved URL is not an error, got {result}"
    );
}

/// The one npm rule that is not obvious: the path keeps the scope and the
/// filename drops it, so `@types/node` is served from `node-20.1.0.tgz`. A
/// tool that got this wrong would send every scoped package's diff to a 404.
#[tokio::test]
async fn a_scoped_npm_package_drops_its_scope_from_the_filename() {
    let result = call(json!({
        "registry": "npm",
        "package": "@types/node",
        "version": "20.1.0",
    }))
    .await;

    assert_eq!(
        result["structuredContent"]["url"],
        "https://registry.npmjs.org/@types/node/-/node-20.1.0.tgz"
    );
}

/// crates.io serves from the static host rather than the API one.
#[tokio::test]
async fn crates_io_resolves_to_the_static_host() {
    let result = call(json!({
        "registry": "crates",
        "package": "serde",
        "version": "1.0.0",
    }))
    .await;

    assert_eq!(
        result["structuredContent"]["url"],
        "https://static.crates.io/crates/serde/serde-1.0.0.crate"
    );
}

/// The structured answer is the contract, and the text beside it is what a
/// client without structured-content support renders. Both have to be there:
/// one of them is what the model reads.
#[tokio::test]
async fn the_answer_is_structured_and_also_readable() {
    let result = call(json!({
        "registry": "crates",
        "package": "serde",
        "version": "1.0.0",
    }))
    .await;

    assert!(result["structuredContent"].is_object(), "got {result}");

    let text = result["content"][0]["text"]
        .as_str()
        .expect("a result should carry a text block too");
    assert!(
        text.contains("static.crates.io"),
        "the text block should carry the answer, got {text}"
    );
}

// ---------------------------------------------------------------------------
// How it fails
// ---------------------------------------------------------------------------

/// A registry this tool cannot resolve is something the model can act on —
/// by asking for a registry it can — so it goes on the channel the model
/// reads. PyPI is the real case: its archive URL is listed only in its own
/// metadata, so there is nothing to build from a package name and a version.
#[tokio::test]
async fn a_registry_this_tool_cannot_resolve_is_a_tool_error() {
    let result = call(json!({
        "registry": "pypi",
        "package": "requests",
        "version": "2.31.0",
    }))
    .await;

    assert_eq!(
        result["isError"],
        json!(true),
        "the model is the one who can pick another registry, got {result}"
    );

    let text = result["content"][0]["text"]
        .as_str()
        .expect("a tool error carries text for the model");
    assert!(
        text.contains("pypi"),
        "the message should name what was asked for, got {text}"
    );
}

/// Arguments that do not validate are the client's mistake, not the model's,
/// so they take the protocol channel and `-32602`. The model never sees this
/// one; the client is expected to correct the call it sent.
#[tokio::test]
async fn arguments_that_do_not_validate_are_a_protocol_error() {
    let answer = post(json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "tools/call",
        "params": {
            "name": TOOL,
            "arguments": { "registry": "npm", "package": "zod" },
            "_meta": meta(),
        },
    }))
    .await;

    assert_eq!(
        answer["error"]["code"], -32602,
        "a missing argument is invalid params, got {answer}"
    );
    assert!(
        answer["result"]["isError"].is_null(),
        "a call that never ran is not a tool that failed, got {answer}"
    );
}

/// A registry that is not one of the three does not reach a handler: the
/// schema declares the enum, so the value fails to validate and the client —
/// which was told the list — is who can fix the call. The refusal names the
/// registries that exist, because a client told only that `go` is wrong has
/// to go and find out what is right.
#[tokio::test]
async fn a_registry_outside_the_enum_is_refused_by_naming_the_ones_that_exist() {
    let answer = post(json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "tools/call",
        "params": {
            "name": TOOL,
            "arguments": { "registry": "go", "package": "logrus", "version": "1.9.3" },
            "_meta": meta(),
        },
    }))
    .await;

    assert_eq!(
        answer["error"]["code"], -32602,
        "an argument outside the declared enum is invalid params, got {answer}"
    );

    let message = answer["error"]["message"]
        .as_str()
        .expect("a protocol error carries a message");
    for known in ["npm", "crates", "pypi"] {
        assert!(
            message.contains(known),
            "the refusal should name `{known}`, got {message}"
        );
    }
}

// ---------------------------------------------------------------------------
// Driving the endpoint
// ---------------------------------------------------------------------------

/// The listed definition of `name`, or a panic naming what was listed.
async fn listed(name: &str) -> Value {
    let answer = post(json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "tools/list",
        "params": { "_meta": meta() },
    }))
    .await;

    let tools = answer["result"]["tools"]
        .as_array()
        .unwrap_or_else(|| panic!("tools/list should answer with an array, got {answer}"))
        .clone();

    tools
        .iter()
        .find(|tool| tool["name"] == name)
        .unwrap_or_else(|| {
            let names: Vec<&str> = tools.iter().filter_map(|t| t["name"].as_str()).collect();
            panic!("`{name}` should be listed, got {names:?}")
        })
        .clone()
}

/// Call the tool with `arguments`, returning the `result` — or panicking with
/// the JSON-RPC error, so a failure says what the server objected to.
///
/// Every answer here is an HTTP `200`, whatever is in the body: a tool that
/// fails has still been called, and the failure is in the result.
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

/// A request as a conforming `2026-07-28` client sends it.
///
/// SEP-2243 repeats what the request is about in headers so an intermediary
/// can route and cache without parsing the body: `Mcp-Method` on every
/// request, and `Mcp-Name` on the ones that name something — the tool for
/// `tools/call`, the URI for `resources/read`. The transport rejects a
/// request that omits one with `-32020`, so a helper that left it out would
/// be testing how this server treats a broken client rather than whether it
/// serves a working one.
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

    let response = router::router()
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
