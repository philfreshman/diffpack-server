mod common;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use common::{Client, CURRENT, PREVIOUS};
use diffpack_server::router;
use serde_json::{json, Value};

fn client(version: &'static str) -> Client {
    Client::routed(router::router).speaking(version)
}

fn body(id: u32, method: &str, params: Value) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params })
}

fn list_tools(id: u32) -> Value {
    body(id, "tools/list", json!({}))
}

#[tokio::test]
async fn a_current_client_lists_tools() {
    let answer = client(CURRENT).respond(list_tools(1)).await;

    assert_eq!(answer.status, StatusCode::OK);
    assert!(
        answer.result()["tools"].is_array(),
        "tools/list should answer with an array of tools, got {}",
        answer.result()
    );
}

#[tokio::test]
async fn a_previous_revision_client_lists_tools() {
    let answer = client(PREVIOUS).respond(list_tools(1)).await;

    assert_eq!(answer.status, StatusCode::OK);
    assert!(answer.result()["tools"].is_array());
}

#[tokio::test]
async fn discover_returns_versions_capabilities_and_identity() {
    let answer = client(CURRENT)
        .respond(body(1, "server/discover", json!({})))
        .await;
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

    let server_info = &result["_meta"]["io.modelcontextprotocol/serverInfo"];
    assert_eq!(server_info["name"], "diffpack");
    assert_eq!(
        server_info["version"],
        env!("CARGO_PKG_VERSION"),
        "the identity should name the build that answered, as /health does"
    );
}

#[tokio::test]
async fn tools_list_carries_a_ttl_and_a_cache_scope() {
    let result = client(CURRENT).respond(list_tools(1)).await.result();

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

    let first = names(&client(CURRENT).respond(list_tools(1)).await.result());
    let second = names(&client(CURRENT).respond(list_tools(2)).await.result());

    assert_eq!(first, second, "two calls should list tools in one order");

    let mut sorted = first.clone();
    sorted.sort();
    assert_eq!(
        first, sorted,
        "the order should be by name, not registration order"
    );
}

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

        let answer = client(CURRENT).send(request).await;

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

#[tokio::test]
async fn no_answer_carries_a_session_id() {
    let offered = "a-session-id-a-client-invented";

    for version in [CURRENT, PREVIOUS] {
        let client = client(version);

        let mut request = client.request(list_tools(1));
        request
            .headers_mut()
            .insert("mcp-session-id", offered.parse().expect("a valid header"));

        let answer = client.send(request).await;

        assert_eq!(answer.status, StatusCode::OK, "on {version}");
        assert_eq!(
            answer.header("mcp-session-id"),
            None,
            "on {version}, the answer should carry no session id"
        );
    }
}

#[tokio::test]
async fn a_disallowed_origin_is_forbidden_and_an_absent_one_is_not() {
    let client = allowing(vec![]);

    let mut with_origin = client.request(list_tools(1));
    with_origin.headers_mut().insert(
        "origin",
        "https://evil.example".parse().expect("a valid header"),
    );

    assert_eq!(
        client.send(with_origin).await.status,
        StatusCode::FORBIDDEN,
        "an origin that is not on the list should be refused"
    );

    assert_eq!(
        client.respond(list_tools(1)).await.status,
        StatusCode::OK,
        "a request with no Origin at all should be served"
    );
}

#[tokio::test]
async fn an_allowed_origin_is_served() {
    let client = allowing(vec!["https://app.example".to_owned()]);

    let mut request = client.request(list_tools(1));
    request.headers_mut().insert(
        "origin",
        "https://app.example".parse().expect("a valid header"),
    );

    assert_eq!(client.send(request).await.status, StatusCode::OK);
}

fn allowing(origins: Vec<String>) -> Client {
    Client::routed(move || {
        router::router_with(
            || Ok(diffpack_server::mcp::Diffpack::new()),
            origins.clone(),
        )
    })
}

#[tokio::test]
async fn health_still_answers() {
    let request = Request::builder()
        .method("GET")
        .uri("/health")
        .header("host", "mcp.diffpack.io")
        .body(Body::empty())
        .expect("the request should build");

    let answer = client(CURRENT).send(request).await;

    assert_eq!(answer.status, StatusCode::OK);
    assert_eq!(answer.json(), diffpack_server::health::body());
}

#[tokio::test]
async fn an_unknown_path_is_not_found() {
    let request = Request::builder()
        .method("GET")
        .uri("/nothing-here")
        .header("host", "mcp.diffpack.io")
        .body(Body::empty())
        .expect("the request should build");

    let answer = client(CURRENT).send(request).await;

    assert_eq!(answer.status, StatusCode::NOT_FOUND);
    assert!(
        answer.json()["error"].is_string(),
        "a 404 should say so in JSON, got {}",
        answer.json()
    );
}
