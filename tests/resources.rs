//! The resources, driven the way a client drives them.
//!
//! One seam: the wire. `resources/list`, `resources/templates/list` and
//! `resources/read` through `router_with` over the fixture archives, which is
//! how `tests/get_diff_tree.rs` drives the tools. A resource is a URI and
//! nothing else — there is no schema a client reads first — so the URI
//! reaching the code that answers it is most of what can break, and a handler
//! test would pass through a URI that never matched.

use axum::body::Body;
use axum::http::Request;
use diffpack_server::mcp::Diffpack;
use diffpack_server::router;
use diffpack_server::tools::Ctx;
use http_body_util::BodyExt;
use serde_json::{json, Value};
use tower::ServiceExt;

const CURRENT: &str = "2026-07-28";

/// The fixture sets this suite is served from, instead of the registries.
const FIXTURES: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/fixtures");

/// The registry catalogue's URI.
const REGISTRIES: &str = "diffpack://registries";

/// A whole comparison, by the handle that names it.
const DIFF_TEMPLATE: &str = "diffpack://diff/{handle}";

/// One file's diff out of that comparison.
const FILE_TEMPLATE: &str = "diffpack://diff/{handle}/file/{path}";

// ---------------------------------------------------------------------------
// What a client is told there is
// ---------------------------------------------------------------------------

/// A client is told there are resources before it asks for any.
///
/// `server/discover` replaced the handshake, so the capabilities there are
/// the only thing that tells a client `resources/list` is worth sending. The
/// list answers either way, which is exactly why this is its own test: a
/// server that served resources and advertised none would pass every other
/// test in this file and reach a client as a server that has none.
#[tokio::test]
async fn a_client_is_told_this_server_has_resources() {
    let answer = post(json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "server/discover",
        "params": { "_meta": meta() },
    }))
    .await;

    assert!(
        answer["result"]["capabilities"]["resources"].is_object(),
        "a server offering resources should say so where a client looks, got {}",
        answer["result"]["capabilities"]
    );
}

/// The catalogue is a resource a client can find without being told.
///
/// `resources/list` is where a client looks, and a server that answers it
/// with nothing is one whose resources exist only for a caller that already
/// knew the URI.
#[tokio::test]
async fn the_registry_catalogue_is_listed() {
    let listed = resources().await;

    let catalogue = listed
        .iter()
        .find(|resource| resource["uri"] == REGISTRIES)
        .unwrap_or_else(|| panic!("`{REGISTRIES}` should be listed, got {listed:?}"));

    assert!(
        catalogue["name"].is_string(),
        "a listed resource carries the name a client shows, got {catalogue}"
    );
}

/// The two diffs are templates, which is a different method.
///
/// `resources/list` carries `Resource`s, which have a `uri` a client can read
/// as it stands; a URI with a `{handle}` in it is not one of those and
/// `resources/templates/list` is where the `2026-07-28` schema puts it. A
/// template listed as a resource would be a URI a client followed literally
/// and got `-32602` for.
#[tokio::test]
async fn both_diff_templates_are_listed() {
    let listed = templates().await;
    let uris: Vec<&str> = listed
        .iter()
        .filter_map(|template| template["uriTemplate"].as_str())
        .collect();

    assert_eq!(
        uris,
        vec![DIFF_TEMPLATE, FILE_TEMPLATE],
        "both diffs should be listed as templates, got {listed:?}"
    );
}

/// And the catalogue is not among them.
///
/// The other half of the split: a URI a client can read as it stands belongs
/// in `resources/list`, and listing it twice would have a client read it
/// twice.
#[tokio::test]
async fn the_catalogue_is_not_a_template() {
    let listed = templates().await;

    assert!(
        !listed
            .iter()
            .any(|template| template["uriTemplate"] == REGISTRIES),
        "`{REGISTRIES}` has nothing to fill in, got {listed:?}"
    );
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Everything `resources/list` answers with.
async fn resources() -> Vec<Value> {
    let answer = post(json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "resources/list",
        "params": { "_meta": meta() },
    }))
    .await;

    answer["result"]["resources"]
        .as_array()
        .unwrap_or_else(|| panic!("resources/list should answer with an array, got {answer}"))
        .clone()
}

/// Everything `resources/templates/list` answers with.
async fn templates() -> Vec<Value> {
    let answer = post(json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "resources/templates/list",
        "params": { "_meta": meta() },
    }))
    .await;

    answer["result"]["resourceTemplates"]
        .as_array()
        .unwrap_or_else(|| {
            panic!("resources/templates/list should answer with an array, got {answer}")
        })
        .clone()
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
async fn post(body: Value) -> Value {
    let (_, answer) = respond(body).await;
    answer
}

/// The same request, with the body the client received beside the answer.
///
/// The bytes rather than the structure, because that is what the response
/// ceiling bounds: what Vercel refuses is the frame this server wrote, and
/// two frames that parse alike can still differ.
async fn respond(body: Value) -> (String, Value) {
    let method = body["method"].as_str().expect("a call names a method");

    let request = Request::builder()
        .method("POST")
        .uri("/mcp")
        .header("host", "mcp.diffpack.io")
        .header("accept", "application/json, text/event-stream")
        .header("content-type", "application/json")
        .header("mcp-protocol-version", CURRENT)
        .header("mcp-method", method)
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

    (String::from_utf8_lossy(&bytes).into_owned(), answer)
}
