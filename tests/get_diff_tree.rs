//! `get_diff_tree`, driven the way an agent drives it.
//!
//! One seam: the wire. `tools/list` for what a client is told and
//! `tools/call` for what it gets back, both through `router_with` over the
//! fixture archives. Testing here rather than at the handler is what keeps
//! the definition and the handler from drifting apart: a schema that stopped
//! describing what the handler reads is a confident wrong answer to a model,
//! and a handler test would go on passing through it.
//!
//! The handle this tool takes comes from `diff_package_versions` wherever one
//! test can get it from there, because the two tools' agreement is the thing
//! worth asserting: a tree that walked to different totals than the summary
//! beside it would make an agent's second call contradict its first.
//!
//! # What is deliberately not asserted here
//!
//! That a walk of a sequence is stable under the response ceiling, and that a
//! cursor of a client's own invention is refused. Those are `src/page.rs`'s
//! and `tests/page.rs` holds them against a generated sequence. What this
//! suite asserts is that this tool goes *through* that module rather than
//! around it — that every node of one comparison is reachable by following
//! the cursors, whatever the page size.
//!
//! That the handle format is what it is. `tests/handle.rs` holds the four
//! ways a handle is refused against hand-built payloads, which is a stronger
//! fixture than any package pair. What is here is the two of them reaching
//! *this* tool's argument, because a tool that took a handle and verified it
//! late would pass every test in that file.
//!
//! That the answer carries `ttlMs` and `cacheScope`. #14 asks for both and a
//! `tools/call` result has nowhere to put them: in the `2026-07-28` schema
//! `CacheableResult` is extended by `DiscoverResult`, the four list results
//! and `ReadResourceResult`, while `CallToolResult` extends plain `Result`.
//! The freshness hint a diff wants is a resource's to carry, which is #16 —
//! the same answer #11, #19 and #42 arrived at before this.

use axum::body::Body;
use axum::http::Request;
use diffpack_server::handle::{DiffHandle, Inputs};
use diffpack_server::mcp::Diffpack;
use diffpack_server::registry::Registry;
use diffpack_server::router;
use diffpack_server::tools::Ctx;
use http_body_util::BodyExt;
use serde_json::{json, Value};
use tower::ServiceExt;

const CURRENT: &str = "2026-07-28";

const TOOL: &str = "get_diff_tree";

/// The tool that mints what this one takes.
const SUMMARY: &str = "diff_package_versions";

/// The fixture sets this suite is served from, instead of the registries.
///
/// The root rather than one seam's directory inside it: `Ctx::fixture` gives
/// every seam a fixture adapter, so nothing this suite builds can reach a
/// registry — including a seam this tool does not use today.
const FIXTURES: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/fixtures");

// ---------------------------------------------------------------------------
// What a client is told
// ---------------------------------------------------------------------------

/// The definition carries what an agent needs to call this correctly having
/// read nothing else, which is #23's question asked of the tool that exists.
#[tokio::test]
async fn the_definition_carries_everything_an_agent_needs() {
    let tool = listed(TOOL).await;

    for field in ["handle", "path", "cursor", "limit"] {
        assert!(
            tool["inputSchema"]["properties"][field].is_object(),
            "the input schema should describe `{field}`, got {}",
            tool["inputSchema"]
        );
    }

    assert_eq!(
        tool["inputSchema"]["required"],
        json!(["handle"]),
        "the handle is the only thing a caller must pass — every other \
         argument narrows an answer that is complete without it: got {}",
        tool["inputSchema"]
    );

    assert_eq!(
        tool["outputSchema"]["type"], "object",
        "a tool answering with structured content declares its shape, got {tool}"
    );

    assert_eq!(
        tool["annotations"]["readOnlyHint"], true,
        "reading a comparison back changes nothing a caller can observe, and \
         a client deciding whether to ask for confirmation reads this, got {}",
        tool["annotations"]
    );
    assert_eq!(
        tool["annotations"]["idempotentHint"], true,
        "a handle names two published versions, which are immutable, so the \
         same page is the same page, got {}",
        tool["annotations"]
    );
    assert_eq!(
        tool["annotations"]["openWorldHint"], true,
        "the handle names a package on a registry, which is a world this \
         server does not control, got {}",
        tool["annotations"]
    );
}

/// The handle this tool takes is described by the module that mints it.
///
/// The same assertion `tests/diff_package_versions.rs` makes about the answer
/// that carries one, and it is here for the half that would break separately:
/// a doc comment on this argument would override the type's description, and
/// what an agent would lose is the sentence saying a handle is passed back
/// unchanged rather than built by hand — in the schema of the tool it is most
/// tempting to build one for.
#[tokio::test]
async fn the_handle_this_tool_takes_is_described_by_the_module_that_mints_it() {
    let tool = listed(TOOL).await;
    let handle = &tool["inputSchema"]["properties"]["handle"];

    assert_eq!(
        handle["type"], "string",
        "a handle is one string on the wire, got {handle}"
    );
    assert_eq!(handle["pattern"], "^d1:[A-Za-z0-9_-]+$", "got {handle}");

    let said = handle["description"]
        .as_str()
        .unwrap_or_else(|| panic!("a described field, got {handle}"));
    assert!(
        said.contains("minted by"),
        "the description should say where a handle comes from: {said}"
    );
    assert!(
        said.contains("not written by hand"),
        "and that it is not one a client builds: {said}"
    );
}

// ---------------------------------------------------------------------------
// What it answers
// ---------------------------------------------------------------------------

/// The whole tree, walked, is the summary the other tool gave.
///
/// The strongest thing this tool can be held to, and the reason the handle
/// below comes from `diff_package_versions` rather than from this test: an
/// agent reads a summary, asks for the tree it describes, and the two have to
/// be the same comparison. A tree that counted differently would make the
/// second call contradict the first with nothing to say which was wrong.
///
/// Files only. The engine gives a directory the sum of what is under it, so
/// counting directories here would report every change once per directory
/// above it — which is what the summary's own totals are documented not to
/// do.
#[tokio::test]
async fn the_whole_tree_walks_to_the_totals_the_summary_reports() {
    let summary = call(
        SUMMARY,
        json!({
            "registry": "npm",
            "package": "diffable",
            "from_version": "1.0.0",
            "to_version": "2.0.0",
        }),
    )
    .await;

    let handle = summary["structuredContent"]["handle"]
        .as_str()
        .unwrap_or_else(|| panic!("the summary carries a handle, got {summary}"))
        .to_owned();

    let nodes = walk(json!({ "handle": handle })).await;

    let mut counted = json!({
        "added": 0, "removed": 0, "modified": 0, "renamed": 0, "unchanged": 0,
        "lines_added": 0, "lines_removed": 0,
    });
    for node in nodes.iter().filter(|node| node["type"] == "file") {
        let status = node["status"].as_str().expect("every node has a status");
        counted[status] = json!(counted[status].as_u64().unwrap_or(0) + 1);
        for (field, counted_as) in [
            ("lines_added", "lines_added"),
            ("lines_removed", "lines_removed"),
        ] {
            let moved = node[field].as_u64().unwrap_or(0);
            counted[counted_as] = json!(counted[counted_as].as_u64().unwrap_or(0) + moved);
        }
    }

    assert_eq!(
        counted, summary["structuredContent"]["totals"],
        "walking the tree completely should count what the summary counted: \
         got {counted} against {summary}"
    );
}

/// The nodes are the comparison's own, in the engine's vocabulary.
///
/// `diffable` 1.0.0 → 2.0.0 has one file of each status, which makes every
/// row below a fact about the fixture rather than about what this tool was
/// asked for. The directory is in the listing too: a tree is what this
/// answers with, and `src` is part of it.
#[tokio::test]
async fn a_node_carries_its_path_its_kind_its_status_and_what_moved() {
    let nodes = walk(json!({ "handle": diffable() })).await;

    assert_eq!(
        nodes,
        vec![
            json!({
                "path": "README.md",
                "type": "file",
                "status": "unchanged",
                "lines_added": 0,
                "lines_removed": 0,
            }),
            json!({
                "path": "src",
                "type": "directory",
                "status": "modified",
                "lines_added": 2,
                "lines_removed": 2,
            }),
            json!({
                "path": "src/added.js",
                "type": "file",
                "status": "added",
                "lines_added": 1,
                "lines_removed": 0,
            }),
            json!({
                "path": "src/index.js",
                "type": "file",
                "status": "modified",
                "lines_added": 1,
                "lines_removed": 1,
            }),
            json!({
                "path": "src/new-name.js",
                "type": "file",
                "status": "renamed",
                "lines_added": 0,
                "lines_removed": 0,
                "old_path": "src/old-name.js",
            }),
            json!({
                "path": "src/removed.js",
                "type": "file",
                "status": "removed",
                "lines_added": 0,
                "lines_removed": 1,
            }),
        ],
        "a directory comes before what is under it and the paths sort, which \
         is the order the comparison itself is in"
    );
}

/// Every node appears exactly once, however small the pages are.
///
/// The property a cursor is for. Two page sizes rather than one: a walk that
/// agrees with itself at one size and not another has a cursor that names a
/// position in an arrangement built for that request.
#[tokio::test]
async fn every_node_appears_exactly_once_across_the_pages() {
    let whole = walk(json!({ "handle": diffable() })).await;

    for limit in [1, 2, 5] {
        let paged = walk(json!({ "handle": diffable(), "limit": limit })).await;
        assert_eq!(
            paged, whole,
            "a walk at {limit} to a page should cover the same nodes in the \
             same order as one that took them all at once"
        );
    }
}

/// A page says how much there is, not how much it handed over.
#[tokio::test]
async fn a_page_reports_the_whole_comparisons_length() {
    let first = call(TOOL, json!({ "handle": diffable(), "limit": 2 })).await;
    let page = &first["structuredContent"];

    assert_eq!(
        page["items"].as_array().map(Vec::len),
        Some(2),
        "got {first}"
    );
    assert_eq!(
        page["total"],
        json!(6),
        "the total is the comparison's length and not the page's, which is \
         what tells an agent there is more: got {first}"
    );
    assert!(
        page["nextCursor"].is_string(),
        "and there is a cursor to follow, got {first}"
    );
}

// ---------------------------------------------------------------------------
// One subtree
// ---------------------------------------------------------------------------

/// `path` answers with what is under that directory and nothing else.
///
/// The directory itself is not in it, which is the half an agent notices: a
/// listing of `src` that began with `src` would make a walk of a subtree
/// disagree with the same nodes read out of a walk of the whole tree.
#[tokio::test]
async fn a_path_returns_that_subtree_and_nothing_outside_it() {
    let nodes = walk(json!({ "handle": diffable(), "path": "src" })).await;

    assert_eq!(
        paths(&nodes),
        [
            "src/added.js",
            "src/index.js",
            "src/new-name.js",
            "src/removed.js",
        ],
        "`README.md` is outside `src` and `src` is not inside itself"
    );
}

/// A directory several levels down is reachable by naming it.
///
/// `many-files` spreads two and a half thousand files over twenty-five
/// directories, so a subtree of it is a real narrowing rather than the whole
/// answer with one row missing — and the pair is the package against itself,
/// which is the only way an archive that exists at one version can be
/// compared at all.
#[tokio::test]
async fn a_subtree_deeper_down_is_reached_by_naming_it() {
    let nodes = walk(json!({
        "handle": handle("many-files", "1.0.0", "1.0.0"),
        "path": "src/07",
    }))
    .await;

    assert_eq!(nodes.len(), 100, "one directory's hundred files");
    let strays: Vec<&str> = paths(&nodes)
        .into_iter()
        .filter(|path| !path.starts_with("src/07/"))
        .collect();
    assert!(strays.is_empty(), "nothing outside the subtree: {strays:?}");
}

/// A trailing slash is allowed and changes nothing.
///
/// An agent writing a directory as `src/` is writing the same directory, and
/// a tool that answered differently would be asking it to know which
/// spelling this server prefers.
#[tokio::test]
async fn a_trailing_slash_on_a_path_makes_no_difference() {
    let bare = walk(json!({ "handle": diffable(), "path": "src" })).await;
    let slashed = walk(json!({ "handle": diffable(), "path": "src/" })).await;

    assert_eq!(bare, slashed);
}

/// A directory the comparison does not have is an empty page, not a refusal.
///
/// Two reasons rather than consistency with `list_package_files` alone. A
/// path that is a file names something real with nothing under it, and the
/// honest answer to "what is inside this" is nothing. And a directory the
/// engine pruned — because a rename took its last file away — is a directory
/// an agent can have read about in the first version and cannot see in the
/// comparison, which is the same answer for a different reason.
#[tokio::test]
async fn a_path_with_nothing_under_it_is_an_empty_page() {
    for path in ["src/index.js", "does-not-exist"] {
        let result = call(TOOL, json!({ "handle": diffable(), "path": path })).await;

        assert_eq!(
            result["isError"],
            json!(false),
            "asking about an empty corner of a comparison is not a failure, \
             got {result}"
        );
        assert_eq!(
            result["structuredContent"]["items"],
            json!([]),
            "got {result}"
        );
        assert_eq!(
            result["structuredContent"]["total"],
            json!(0),
            "and the total says so rather than the page being short: got {result}"
        );
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// A handle for `diffable` 1.0.0 → 2.0.0, minted rather than fetched.
///
/// The tool that mints one is called where the *agreement* between the two is
/// what is being asserted. Everywhere else a handle is just the argument, and
/// minting it here is one call rather than two.
fn diffable() -> String {
    handle("diffable", "1.0.0", "2.0.0")
}

/// A handle for one npm comparison, at the defaults `diff_package_versions`
/// would have used.
fn handle(package: &str, from: &str, to: &str) -> String {
    DiffHandle::mint(Inputs {
        registry: Registry::Npm,
        package: package.to_owned(),
        from_version: from.to_owned(),
        to_version: to.to_owned(),
        similarity_threshold: 0.75,
        ignore_whitespace: false,
    })
    .encode()
}

/// The paths of `nodes`, in the order they came back.
fn paths(nodes: &[Value]) -> Vec<&str> {
    nodes
        .iter()
        .map(|node| {
            node["path"]
                .as_str()
                .unwrap_or_else(|| panic!("every node has a path, got {node}"))
        })
        .collect()
}

/// Every node `arguments` selects, by following the cursors to the end.
///
/// What an agent does with a paginated answer, and the only way to ask this
/// tool for a whole comparison. The page's own `total` is checked against
/// what came back, so a walk that stopped early fails here rather than in
/// whichever assertion happened to read the last node.
async fn walk(arguments: Value) -> Vec<Value> {
    let mut collected = Vec::new();
    let mut cursor: Option<String> = None;

    loop {
        let mut asked = arguments.clone();
        if let Some(cursor) = &cursor {
            asked["cursor"] = json!(cursor);
        }

        let result = call(TOOL, asked).await;
        let page = &result["structuredContent"];

        let items = page["items"]
            .as_array()
            .unwrap_or_else(|| panic!("a page carries items, got {result}"));
        collected.extend(items.iter().cloned());

        match page["nextCursor"].as_str() {
            Some(next) => cursor = Some(next.to_owned()),
            None => {
                assert_eq!(
                    page["total"],
                    json!(collected.len()),
                    "a walk that followed every cursor should have every node \
                     the last page said there were: got {result}"
                );
                return collected;
            }
        }
    }
}

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

/// Call `tool` with `arguments`, returning the `result` — or panicking with
/// the JSON-RPC error, so a failure says what the server objected to.
async fn call(tool: &str, arguments: Value) -> Value {
    let answer = post(json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "tools/call",
        "params": { "name": tool, "arguments": arguments, "_meta": meta() },
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
/// through the service factory `router_with` takes, which builds the `Ctx`
/// every handler is handed. A test that reached around it would be testing a
/// path production does not take.
async fn post(body: Value) -> Value {
    let (_, answer) = respond(body).await;
    answer
}

/// The same request, with the bytes the client received beside the answer.
///
/// The length is what the response ceiling is about, and it is not visible
/// from a parsed body: what Vercel refuses is the frame this server wrote,
/// not the structure inside it.
async fn respond(body: Value) -> (usize, Value) {
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

    let answer = serde_json::from_slice(&bytes).unwrap_or_else(|e| {
        panic!(
            "expected a JSON body ({status}), got {e}: {}",
            String::from_utf8_lossy(&bytes)
        )
    });

    (bytes.len(), answer)
}
