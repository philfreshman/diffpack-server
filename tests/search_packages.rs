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
use diffpack_server::error::Failure;
use diffpack_server::mcp::Diffpack;
use diffpack_server::registry::Registry;
use diffpack_server::router;
use diffpack_server::tools::search_packages::{Args, SearchPackages};
use diffpack_server::tools::{Ctx, Tool};
use http_body_util::BodyExt;
use serde_json::{json, Value};
use tower::ServiceExt;

const CURRENT: &str = "2026-07-28";

const TOOL: &str = "search_packages";

/// The fixture sets this suite is served, instead of the registries.
///
/// The root rather than one seam's directory inside it: `Ctx::fixture` gives
/// each seam its own, so a suite naming the root cannot wire one of them to
/// another's set.
const FIXTURES: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/fixtures");

// ---------------------------------------------------------------------------
// At the handler
// ---------------------------------------------------------------------------

/// Which failure a quiet source produces, which is the question the wire
/// cannot answer: `isError` and a sentence look the same whichever variant
/// built them, and the variant is what decides whether the sentence says
/// "try again".
#[tokio::test]
async fn the_handler_returns_the_failure_that_says_the_source_is_unwell() {
    let failure = SearchPackages::call(
        Args {
            registry: Registry::Npm,
            query: "outage".to_owned(),
            limit: None,
        },
        &Ctx::fixture(FIXTURES),
    )
    .await
    .expect_err("this source is not answering");

    match failure {
        Failure::Unavailable { registry, .. } => assert_eq!(registry, "npm"),
        other => panic!("a source that did not answer should say so, got {other:?}"),
    }
}

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

/// A limit is asked *of the source*, not applied to what comes back. The
/// fixture set is keyed by URL, so this passes only if the number a caller
/// gave reached the request npm was sent — which is the difference between
/// politeness about how much an agent reads and politeness about how much
/// somebody else's server is made to send.
#[tokio::test]
async fn a_limit_is_what_the_source_is_asked_for() {
    let result = call(json!({
        "registry": "npm",
        "query": "zod",
        "limit": 1,
    }))
    .await;

    let page = &result["structuredContent"];
    assert_eq!(
        page["items"].as_array().map(Vec::len),
        Some(1),
        "one hit was asked for, got {page}"
    );
    assert_eq!(
        page["items"][0]["name"], "zod",
        "and it is the best one, got {page}"
    );
    assert_eq!(
        page["total"], 1,
        "a total is the length of what there was to page, which is what a \
         source asked for one hit returned, got {page}"
    );
}

/// A query nobody publishes anything for is an empty answer and not a
/// failure: there is nothing for a model to fix, and an `isError` here would
/// have it apologising for a registry's silence or retrying a search that
/// worked.
#[tokio::test]
async fn a_query_that_matches_nothing_is_an_empty_page() {
    let result = call(json!({
        "registry": "npm",
        "query": "zzqqxxnotapackage",
    }))
    .await;

    assert_eq!(
        result["structuredContent"]["items"],
        json!([]),
        "got {result}"
    );
    assert_eq!(result["structuredContent"]["total"], 0, "got {result}");
    assert_eq!(
        result["isError"],
        json!(false),
        "finding nothing is an answer, got {result}"
    );
}

/// A blank query finds nothing, and finds it without asking anybody.
///
/// It is the one query the three sources would each answer differently:
/// every name in PyPI's index begins with nothing, so matching it there
/// returns a page of whatever happens to be shortest, and npm and crates.io
/// each have their own idea of a query-less query. None of those is an
/// answer to "which packages are called this", so the tool gives the one
/// that is.
///
/// Driven against the registry whose fixture set would have answered, so a
/// pass is the short circuit rather than a URL the set happens not to carry.
#[tokio::test]
async fn a_blank_query_finds_nothing_rather_than_everything() {
    for query in ["", "   "] {
        let result = call(json!({ "registry": "pypi", "query": query })).await;

        assert_eq!(
            result["structuredContent"]["items"],
            json!([]),
            "a blank query is not a request for the whole index, got {result}"
        );
        assert_eq!(result["structuredContent"]["total"], 0, "got {result}");
        assert_eq!(
            result["isError"],
            json!(false),
            "asking for nothing is not an error, got {result}"
        );
    }
}

/// A query with spaces around it is the query without them.
///
/// A model that pasted a name out of a sentence should get the same answer
/// as one that typed it, and the alternative is a percent-encoded space in
/// the URL that npm matches nothing for.
#[tokio::test]
async fn a_query_is_trimmed_before_it_reaches_a_registry() {
    let result = call(json!({ "registry": "npm", "query": "  zod  " })).await;

    assert_eq!(
        result["structuredContent"]["items"][0]["name"], "zod",
        "the fixture set is keyed by the untrimmed URL's absence, got {result}"
    );
}

/// A source that is not answering is a tool error the model reads, not a
/// protocol error it never sees: the remedy is to try again or to search
/// somewhere else, and only the model can choose. The message says which
/// registry went quiet and that trying again is worth it, which is the
/// distinction #7 exists to keep.
#[tokio::test]
async fn a_source_that_is_not_answering_is_a_tool_error_a_model_can_retry() {
    let result = call(json!({
        "registry": "npm",
        "query": "outage",
    }))
    .await;

    assert_eq!(
        result["isError"],
        json!(true),
        "a source that did not answer is a failure, got {result}"
    );

    let said = result["content"][0]["text"].as_str().unwrap_or_default();
    assert!(
        said.contains("npm"),
        "the message should name the registry that went quiet, got {said:?}"
    );
    assert!(
        said.contains("Try again"),
        "and should say that trying again is worth it, got {said:?}"
    );
}

/// And it takes out that registry's search rather than the tool. An agent
/// that met one failing source and concluded `search_packages` was broken
/// would stop using it for the two registries that are answering perfectly
/// well.
#[tokio::test]
async fn one_source_being_down_leaves_the_others_answering() {
    let down = call(json!({ "registry": "npm", "query": "outage" })).await;
    assert_eq!(down["isError"], json!(true), "got {down}");

    let up = call(json!({ "registry": "crates", "query": "serde" })).await;
    assert_eq!(
        up["structuredContent"]["items"][0]["name"], "serde",
        "the next call to another registry answers as it always did, got {up}"
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
        || Ok(Diffpack::with_ctx(Ctx::fixture(FIXTURES))),
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
