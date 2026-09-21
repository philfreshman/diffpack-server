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
// What a client is told
// ---------------------------------------------------------------------------

/// The definition carries what an agent needs to call this correctly having
/// read nothing else, which is #23's question asked of the tool that exists.
#[tokio::test]
async fn the_definition_carries_everything_an_agent_needs() {
    let tool = listed(TOOL).await;

    for field in ["registry", "query", "limit"] {
        assert!(
            tool["inputSchema"]["properties"][field].is_object(),
            "the input schema should describe `{field}`, got {}",
            tool["inputSchema"]
        );
    }
    for required in ["registry", "query"] {
        assert!(
            tool["inputSchema"]["required"]
                .as_array()
                .is_some_and(|fields| fields.iter().any(|field| field == required)),
            "`{required}` is not optional, got {}",
            tool["inputSchema"]
        );
    }
    assert!(
        tool["inputSchema"]["required"]
            .as_array()
            .is_some_and(|fields| !fields.iter().any(|field| field == "limit")),
        "a limit that has a default is not something a caller has to supply, got {}",
        tool["inputSchema"]
    );

    assert_eq!(
        tool["inputSchema"]["properties"]["registry"]["enum"],
        json!(["npm", "crates", "pypi"]),
        "the enum comes from the registry module rather than from prose here, got {tool}"
    );

    for field in ["items", "total"] {
        assert!(
            tool["outputSchema"]["properties"][field].is_object(),
            "the output schema should describe `{field}`, got {}",
            tool["outputSchema"]
        );
    }
    for field in ["name", "version", "description"] {
        assert!(
            tool["outputSchema"]["properties"]["items"]["items"]["properties"][field].is_object(),
            "a hit should describe `{field}`, got {}",
            tool["outputSchema"]
        );
    }

    assert_eq!(
        tool["annotations"]["readOnlyHint"], true,
        "searching changes nothing, and a client deciding whether to ask for \
         confirmation reads this, got {}",
        tool["annotations"]
    );
    assert_eq!(
        tool["annotations"]["openWorldHint"], true,
        "what answers a query is whatever the registry has, got {}",
        tool["annotations"]
    );
}

/// The one hint this tool answers differently from every other one here, and
/// the reason is the whole difference between a search and everything else
/// this server does: a published version's contents cannot change, and what a
/// registry has today can. An agent that cached a search on this hint would
/// be answering tomorrow's question with yesterday's index.
#[tokio::test]
async fn a_search_does_not_claim_the_same_answer_twice() {
    let tool = listed(TOOL).await;

    assert_eq!(
        tool["annotations"]["idempotentHint"], false,
        "a registry's index moves under a search, got {}",
        tool["annotations"]
    );
}

/// The asymmetry an agent cannot infer from the schema: a hit's version and
/// description are absent for PyPI and present for the other two, because
/// PyPI's index carries neither. A tool that left this to be discovered
/// would have an agent deciding a PyPI package has no releases.
#[tokio::test]
async fn the_description_says_which_registry_answers_with_less() {
    let tool = listed(TOOL).await;
    let said = tool["description"].as_str().unwrap_or_default();

    assert!(
        said.contains("PyPI"),
        "the description should name the registry that answers with less, got {said:?}"
    );
    assert!(
        said.contains("nothing else") || said.contains("a name alone"),
        "and should say what it leaves out, got {said:?}"
    );
}

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

/// crates.io, which answers in a shape of its own and with three version
/// fields that need not agree. The version a hit carries is the one
/// crates.io would install, which is what `tests/registry.rs` holds the
/// reading to; what this asserts is that the tool goes through that reading
/// rather than around it.
#[tokio::test]
async fn a_crates_io_search_answers_with_the_crates_it_found() {
    let result = call(json!({
        "registry": "crates",
        "query": "serde",
    }))
    .await;

    let items = &result["structuredContent"]["items"];
    assert_eq!(
        items[0],
        json!({
            "name": "serde",
            "version": "1.0.229",
            "description": "A generic serialization/deserialization framework",
        }),
        "got {items}"
    );
}

/// PyPI, whose answer is the index of everything it publishes rather than a
/// reply to a query. What a caller sees is the same shape as the other two,
/// minus the fields PyPI's index does not carry — which is the one asymmetry
/// the tool's description tells an agent about out loud.
#[tokio::test]
async fn a_pypi_search_answers_with_names_and_says_no_more_than_that() {
    let result = call(json!({
        "registry": "pypi",
        "query": "yaml",
    }))
    .await;

    let items = &result["structuredContent"]["items"];
    assert_eq!(
        items[0],
        json!({ "name": "yaml" }),
        "a PyPI hit is a name, and no empty version or description beside it, got {items}"
    );
    assert_eq!(
        items[1]["name"], "yamldown",
        "and the rest are ranked against the query here — `yamldown` before \
         `yamllint` because the two are the same length, got {items}"
    );
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
