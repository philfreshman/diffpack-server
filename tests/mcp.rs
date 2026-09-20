//! The MCP endpoint, driven the way a client drives it.
//!
//! These tests send real requests through the real router and read real
//! responses back. Nothing reaches into `rmcp` — the whole point of depending
//! on the SDK is that its internals are not our contract, and a test that
//! asserted against them would pass while the wire was broken and fail on an
//! SDK upgrade that changed nothing a client can see.
//!
//! What *is* our contract is the HTTP surface: which methods answer, which
//! status codes come back, which headers appear and which never do. That is
//! what is pinned here.
//!
//! No port is bound. `tower`'s `oneshot` hands a request straight to the
//! router, which is the same code path `vercel_runtime` drives in production
//! minus the socket.

use axum::body::Body;
use axum::http::{HeaderMap, Request, StatusCode};
use diffpack_server::router;
use http_body_util::BodyExt;
use serde_json::{json, Value};
use tower::ServiceExt;

/// The revision this server is written against: no protocol-level sessions,
/// no `initialize` handshake, `server/discover` mandatory.
const CURRENT: &str = "2026-07-28";

/// The previous revision, still spoken by clients that have not caught up.
/// One endpoint has to serve both, so every test that matters is run against
/// this one too.
const PREVIOUS: &str = "2025-11-25";

/// What a client sends and what it gets back, with the body left as bytes:
/// not every answer is JSON (a `405` is plain text), and a test that assumed
/// otherwise would report a parse failure instead of the status it meant to
/// check.
struct Answer {
    status: StatusCode,
    headers: HeaderMap,
    body: Vec<u8>,
}

impl Answer {
    fn json(&self) -> Value {
        serde_json::from_slice(&self.body).unwrap_or_else(|e| {
            panic!(
                "expected a JSON body, got {e}: {}",
                String::from_utf8_lossy(&self.body)
            )
        })
    }

    /// The `result` of a successful JSON-RPC response, or a panic naming the
    /// error — so a test that fails says what the server objected to rather
    /// than `None`.
    fn result(&self) -> Value {
        let body = self.json();
        if let Some(error) = body.get("error") {
            panic!("expected a result, got JSON-RPC error {error}");
        }
        body["result"].clone()
    }

    fn header(&self, name: &str) -> Option<&str> {
        self.headers.get(name).and_then(|v| v.to_str().ok())
    }
}

/// A POST to `/mcp` calling `method`, as a conforming client of `version`
/// sends it.
///
/// Everything below is the client's obligation under the transport, not this
/// server's leniency, which is why the builder always meets it: a request
/// that skipped any of it would be testing how we treat a broken client
/// rather than whether we serve a working one.
///
/// * `Accept` names both `application/json` and `text/event-stream`, and
///   `Content-Type` is JSON.
/// * From `2026-07-28`, SEP-2243 repeats the method in an `Mcp-Method`
///   header, so an intermediary can route without parsing the body.
/// * From `2026-07-28`, the protocol version and the client's capabilities
///   ride in each request's `_meta`. Dropping the `initialize` handshake had
///   to put what it carried somewhere, and with no session to have agreed
///   them in earlier, that somewhere is every request.
fn mcp_post(version: &str, id: u32, method: &str, params: Value) -> Request<Body> {
    // ISO-8601 dates sort lexicographically, so a string comparison is the
    // "this revision and later" the spec's own wording means.
    let current = version >= CURRENT;

    let mut params = params;
    if current {
        params["_meta"] = json!({
            "io.modelcontextprotocol/protocolVersion": version,
            "io.modelcontextprotocol/clientCapabilities": {},
        });
    }

    let mut request = Request::builder()
        .method("POST")
        .uri("/mcp")
        .header("host", "mcp.diffpack.io")
        .header("accept", "application/json, text/event-stream")
        .header("content-type", "application/json")
        .header("mcp-protocol-version", version);

    if current {
        request = request.header("mcp-method", method);
    }

    let body = json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params });

    request
        .body(Body::from(body.to_string()))
        .expect("the request should build")
}

/// The call every test that is not about a particular method makes.
fn list_tools(version: &str, id: u32) -> Request<Body> {
    mcp_post(version, id, "tools/list", json!({}))
}

/// Drive `request` through the deployed router.
async fn ask(request: Request<Body>) -> Answer {
    ask_of(router::router(), request).await
}

/// Drive `request` through a router the test built itself — the seam the
/// origin tests need, because the allowed list is the thing under test.
async fn ask_of(router: axum::Router, request: Request<Body>) -> Answer {
    let response = router
        .oneshot(request)
        .await
        .expect("the router answers every request");

    let status = response.status();
    let headers = response.headers().clone();
    let body = response
        .into_body()
        .collect()
        .await
        .expect("the body should read")
        .to_bytes()
        .to_vec();

    Answer {
        status,
        headers,
        body,
    }
}

/// `tools/list` succeeds on the current revision. An empty list is a correct
/// answer until #11 lands a tool; what is being pinned is that a client can
/// connect and ask at all.
#[tokio::test]
async fn a_current_client_lists_tools() {
    let answer = ask(list_tools(CURRENT, 1)).await;

    assert_eq!(answer.status, StatusCode::OK);
    assert!(
        answer.result()["tools"].is_array(),
        "tools/list should answer with an array of tools, got {}",
        answer.result()
    );
}

/// The same, from a client that has not moved to the current revision. This
/// is the compatibility claim the whole design rests on: one stateless
/// endpoint, every client we care about.
#[tokio::test]
async fn a_previous_revision_client_lists_tools() {
    let answer = ask(list_tools(PREVIOUS, 1)).await;

    assert_eq!(answer.status, StatusCode::OK);
    assert!(answer.result()["tools"].is_array());
}

/// `server/discover` replaced the `initialize` handshake, so it is the only
/// way a client learns what it is talking to. All three answers matter: the
/// versions bound what can be negotiated, the capabilities say which requests
/// are worth sending, and the identity is what a user sees in a client's
/// server list.
#[tokio::test]
async fn discover_returns_versions_capabilities_and_identity() {
    let answer = ask(mcp_post(CURRENT, 1, "server/discover", json!({}))).await;
    assert_eq!(answer.status, StatusCode::OK);

    let result = answer.result();

    let versions: Vec<&str> = result["supportedVersions"]
        .as_array()
        .expect("discover should list supported versions")
        .iter()
        .map(|v| v.as_str().expect("a version is a string"))
        .collect();
    assert!(
        versions.contains(&CURRENT) && versions.contains(&PREVIOUS),
        "discover should advertise both revisions this server answers, got {versions:?}"
    );

    assert!(
        result["capabilities"]["tools"].is_object(),
        "a server whose whole purpose is tools should advertise the tools capability, got {}",
        result["capabilities"]
    );

    // The identity travels in `_meta` under the key the spec reserves for it.
    let server_info = &result["_meta"]["io.modelcontextprotocol/serverInfo"];
    assert_eq!(server_info["name"], "diffpack");
    assert_eq!(
        server_info["version"],
        env!("CARGO_PKG_VERSION"),
        "the identity should name the build that answered, as /health does"
    );
}

/// A `tools/list` result has to carry `ttlMs` and `cacheScope`: from
/// `2026-07-28` they are how a client knows whether it may reuse the list
/// instead of asking again, and on a serverless function every avoided round
/// trip is an avoided cold start.
#[tokio::test]
async fn tools_list_carries_a_ttl_and_a_cache_scope() {
    let result = ask(list_tools(CURRENT, 1))
        .await
        .result();

    assert!(
        result["ttlMs"].as_u64().is_some(),
        "tools/list should say how long its answer stays fresh, got {result}"
    );
    assert_eq!(
        result["cacheScope"], "public",
        "the tool list is the same for every caller — this server has no \
         authorization contexts to keep apart"
    );
}

/// The spec asks for a deterministic order so that a client can cache the
/// list and compare it cheaply. With no tools registered this is vacuous, and
/// that is the point: it is here so that the first tool to land cannot land
/// in whatever order it was registered in.
#[tokio::test]
async fn tools_are_listed_in_a_deterministic_order() {
    let names = |result: &Value| -> Vec<String> {
        result["tools"]
            .as_array()
            .expect("tools should be an array")
            .iter()
            .map(|t| t["name"].as_str().expect("a tool has a name").to_owned())
            .collect()
    };

    let first = names(&ask(list_tools(CURRENT, 1)).await.result());
    let second = names(&ask(list_tools(CURRENT, 2)).await.result());

    assert_eq!(first, second, "two calls should list tools in one order");

    let mut sorted = first.clone();
    sorted.sort();
    assert_eq!(
        first, sorted,
        "the order should be by name, not registration order"
    );
}

/// The transport says a server that does not offer a server-initiated stream
/// answers `GET` with `405`, and the same for `DELETE`, which only ever
/// existed to end a session. A serverless function has neither: there is no
/// process to hold a stream open and no session to end.
#[tokio::test]
async fn get_and_delete_are_method_not_allowed() {
    for method in ["GET", "DELETE"] {
        let request = Request::builder()
            .method(method)
            .uri("/mcp")
            .header("host", "mcp.diffpack.io")
            .header("accept", "application/json, text/event-stream")
            .body(Body::empty())
            .expect("the request should build");

        let answer = ask(request).await;

        assert_eq!(
            answer.status,
            StatusCode::METHOD_NOT_ALLOWED,
            "{method} /mcp should be refused"
        );
        assert_eq!(
            answer.header("allow"),
            Some("POST"),
            "a 405 has to say what is allowed instead"
        );
    }
}

/// No session id, ever — not echoed, not minted. The function that answers
/// the next request is a different process with no memory of this one, so a
/// session id would be a promise the deployment cannot keep: a client that
/// believed it would send it back and be told the session is gone.
#[tokio::test]
async fn no_answer_carries_a_session_id() {
    let offered = "a-session-id-a-client-invented";

    for version in [CURRENT, PREVIOUS] {
        let mut request = list_tools(version, 1);
        request
            .headers_mut()
            .insert("mcp-session-id", offered.parse().expect("a valid header"));

        let answer = ask(request).await;

        assert_eq!(answer.status, StatusCode::OK, "on {version}");
        assert_eq!(
            answer.header("mcp-session-id"),
            None,
            "on {version}, the answer should carry no session id"
        );
    }
}

/// `Origin` is the one header a browser cannot be made to lie about, so it is
/// the whole of the cross-origin defence for a public endpoint. A page that
/// is not on the list must not be able to drive this server with a user's
/// credentials.
///
/// A request with no `Origin` is not a browser request and is accepted: every
/// MCP client we care about — Claude Code, Claude Desktop, Codex — is a local
/// process that sends none.
#[tokio::test]
async fn a_disallowed_origin_is_forbidden_and_an_absent_one_is_not() {
    let router = || router::router_with(|| Ok(diffpack_server::mcp::Diffpack::new()), vec![]);

    let mut with_origin = list_tools(CURRENT, 1);
    with_origin.headers_mut().insert(
        "origin",
        "https://evil.example".parse().expect("a valid header"),
    );

    assert_eq!(
        ask_of(router(), with_origin).await.status,
        StatusCode::FORBIDDEN,
        "an origin that is not on the list should be refused"
    );

    assert_eq!(
        ask_of(router(), list_tools(CURRENT, 1))
            .await
            .status,
        StatusCode::OK,
        "a request with no Origin at all should be served"
    );
}

/// The list is configuration, not a constant: a browser-based client can be
/// let in without a code change, which is what #25 will need.
#[tokio::test]
async fn an_allowed_origin_is_served() {
    let router = router::router_with(
        || Ok(diffpack_server::mcp::Diffpack::new()),
        vec!["https://app.example".to_owned()],
    );

    let mut request = list_tools(CURRENT, 1);
    request.headers_mut().insert(
        "origin",
        "https://app.example".parse().expect("a valid header"),
    );

    assert_eq!(ask_of(router, request).await.status, StatusCode::OK);
}

/// `/health` predates the protocol and outlives it: it is what #8 deploys
/// against, and a change to the router that stranded it would take the only
/// check that does not need an MCP client with it.
#[tokio::test]
async fn health_still_answers() {
    let request = Request::builder()
        .method("GET")
        .uri("/health")
        .header("host", "mcp.diffpack.io")
        .body(Body::empty())
        .expect("the request should build");

    let answer = ask(request).await;

    assert_eq!(answer.status, StatusCode::OK);
    assert_eq!(answer.json(), diffpack_server::health::body());
}

/// Every path reaches this function (`vercel.json` rewrites `/(.*)` to it), so
/// the router owns the 404 as well. It is JSON because everything else this
/// endpoint answers is, and a client parsing the body should not have to
/// special-case one route.
#[tokio::test]
async fn an_unknown_path_is_not_found() {
    let request = Request::builder()
        .method("GET")
        .uri("/nothing-here")
        .header("host", "mcp.diffpack.io")
        .body(Body::empty())
        .expect("the request should build");

    let answer = ask(request).await;

    assert_eq!(answer.status, StatusCode::NOT_FOUND);
    assert!(
        answer.json()["error"].is_string(),
        "a 404 should say so in JSON, got {}",
        answer.json()
    );
}
