use std::time::Duration;

mod common;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use common::{Answer, Client, FIXTURES};
use diffpack_server::entries::Entries;
use diffpack_server::router;
use diffpack_server::store::{Memory, Operation};
use diffpack_server::tools::Ctx;
use serde_json::{json, Value};

const NEVER_WRITTEN: &str = "282467a6bd7b210db4077e71bcf3901550b219852386085452f5739c6b6c436c";

fn diffable() -> Value {
    json!({
        "registry": "npm",
        "package": "diffable",
        "from_version": "1.0.0",
        "to_version": "2.0.0",
    })
}

#[tokio::test]
async fn an_entry_the_server_wrote_is_served_by_its_diff_id() {
    let store = Memory::new();
    let computed = compare(&store, diffable()).await;

    let served = fetch(&store, &format!("/diffs/v1/{}", diff_id_of(&computed))).await;

    assert_eq!(served.status, StatusCode::OK);
    let body = served.json();
    assert_eq!(
        body["tree"],
        written_meta(&store, &computed)["tree"],
        "the tree served is the one the server wrote, got {body}"
    );
    assert!(
        body["patches"]
            .as_object()
            .is_some_and(|patches| !patches.is_empty()),
        "a diff with changes is served with its patches, got {body}"
    );
}

#[tokio::test]
async fn a_hit_is_remembered_for_good_because_its_diff_id_names_its_contents() {
    let store = Memory::new();
    let computed = compare(&store, diffable()).await;

    let served = fetch(&store, &format!("/diffs/v1/{}", diff_id_of(&computed))).await;

    assert_eq!(
        served.header("cache-control"),
        Some("public, max-age=31536000, immutable")
    );
}

#[tokio::test]
async fn a_diff_id_nothing_was_written_under_is_a_miss_nobody_remembers() {
    let served = fetch(&Memory::new(), &format!("/diffs/v1/{NEVER_WRITTEN}")).await;

    assert_eq!(served.status, StatusCode::NOT_FOUND);
    assert_eq!(
        served.header("cache-control"),
        Some("no-store"),
        "a miss today can be a hit once the entry is written"
    );
}

#[tokio::test]
async fn any_page_may_read_a_hit_or_a_miss_and_none_may_send_credentials() {
    let store = Memory::new();
    let computed = compare(&store, diffable()).await;

    for path in [
        format!("/diffs/v1/{}", diff_id_of(&computed)),
        format!("/diffs/v1/{NEVER_WRITTEN}"),
    ] {
        let served = fetch(&store, &path).await;

        assert_eq!(
            served.header("access-control-allow-origin"),
            Some("*"),
            "{path}"
        );
        assert_eq!(
            served.header("access-control-allow-credentials"),
            None,
            "{path}"
        );
    }
}

#[tokio::test]
async fn half_an_entry_is_a_miss() {
    let store = Memory::new();
    let computed = compare(&store, diffable()).await;
    store.forget(&format!("diffs/v1/{}/patches.json", diff_id_of(&computed)));

    let served = fetch(&store, &format!("/diffs/v1/{}", diff_id_of(&computed))).await;

    assert_eq!(served.status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn a_store_that_cannot_be_read_is_a_miss_and_not_a_failure() {
    let store = Memory::new();
    let computed = compare(&store, diffable()).await;
    let failing = Memory::failing(&store, Operation::Read);

    let served = fetch(&failing, &format!("/diffs/v1/{}", diff_id_of(&computed))).await;

    assert_eq!(served.status, StatusCode::NOT_FOUND);
    assert_eq!(served.header("cache-control"), Some("no-store"));
}

#[tokio::test]
async fn only_a_diff_id_names_an_entry() {
    let store = Memory::new();
    let computed = compare(&store, diffable()).await;
    let diff_id = diff_id_of(&computed);

    for not_a_diff_id in [
        diff_id.to_uppercase(),
        diff_id[..63].to_owned(),
        format!("{diff_id}0"),
        "..%2F..%2Ftests".to_owned(),
    ] {
        let served = fetch(&store, &format!("/diffs/v1/{not_a_diff_id}")).await;

        assert_eq!(served.status, StatusCode::NOT_FOUND, "{not_a_diff_id}");
    }
}

#[tokio::test]
async fn an_entry_says_which_comparison_it_is() {
    let store = Memory::new();
    let computed = compare(&store, diffable()).await;

    let body = fetch(&store, &format!("/diffs/v1/{}", diff_id_of(&computed)))
        .await
        .json();

    assert_eq!(body["schema"], 1);
    assert_eq!(body["registry"], "npm");
    assert_eq!(body["package"], "diffable");
    assert_eq!(body["from"], "1.0.0");
    assert_eq!(body["to"], "2.0.0");
    assert_eq!(body["engine"], diffpack_server::engine::VERSION);
    assert_eq!(body["similarity_threshold"], 0.75);
    assert_eq!(body["ignore_whitespace"], false);
}

#[tokio::test]
async fn an_entry_over_the_response_ceiling_is_served_without_its_patches() {
    let store = Memory::new();
    let computed = compare(&store, diffable()).await;
    let path = format!("/diffs/v1/{}", diff_id_of(&computed));
    let whole = fetch(&store, &path).await.body_len();
    let tree_alone = serde_json::to_vec(&written_meta(&store, &computed))
        .expect("a meta.json reserialises")
        .len();
    assert!(
        tree_alone < whole,
        "the fixture's patches have to weigh something"
    );

    let served = fetch_capped(&store, &path, whole - 1).await;

    assert_eq!(served.status, StatusCode::OK);
    let body = served.json();
    assert_eq!(body["tree"], written_meta(&store, &computed)["tree"]);
    assert_eq!(body["patches"], json!({}), "got {body}");
}

#[tokio::test]
async fn an_entry_whose_tree_alone_is_over_the_response_ceiling_is_a_miss() {
    let store = Memory::new();
    let computed = compare(&store, diffable()).await;

    let served = fetch_capped(&store, &format!("/diffs/v1/{}", diff_id_of(&computed)), 16).await;

    assert_eq!(served.status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn the_deployed_router_serves_entries() {
    let served = Client::routed(router::router)
        .send(get(&format!("/diffs/v1/{NEVER_WRITTEN}")))
        .await;

    assert_eq!(served.status, StatusCode::NOT_FOUND);
    assert_eq!(
        served.header("access-control-allow-origin"),
        Some("*"),
        "a miss from the entries route and not the catch-all one"
    );
}

async fn compare(store: &Memory, arguments: Value) -> Value {
    let answer = Client::over(Ctx::fixture(FIXTURES).storing_in(store.store()))
        .call("diff_package_versions", arguments)
        .await;
    settles(store, 2).await;
    answer
}

async fn fetch(store: &Memory, path: &str) -> Answer {
    let store = store.clone();
    Client::routed(move || {
        let store = store.clone();
        Entries::over(move || store.store()).routes()
    })
    .send(get(path))
    .await
}

async fn fetch_capped(store: &Memory, path: &str, ceiling: usize) -> Answer {
    let store = store.clone();
    Client::routed(move || {
        let store = store.clone();
        Entries::over(move || store.store())
            .capping_responses_at(ceiling)
            .routes()
    })
    .send(get(path))
    .await
}

fn get(path: &str) -> Request<Body> {
    Request::builder()
        .method("GET")
        .uri(path)
        .header("origin", "https://diffpack.io")
        .body(Body::empty())
        .expect("a GET with a path builds")
}

fn written_meta(store: &Memory, computed: &Value) -> Value {
    let pathname = format!("diffs/v1/{}/meta.json", diff_id_of(computed));
    let bytes = store
        .blob(&pathname)
        .unwrap_or_else(|| panic!("the server wrote {pathname}"));
    serde_json::from_slice(&bytes).expect("a written meta.json is JSON")
}

fn diff_id_of(answer: &Value) -> &str {
    answer["structuredContent"]["diff_id"]
        .as_str()
        .unwrap_or_else(|| panic!("the answer names its diff_id, got {answer}"))
}

async fn settles(store: &Memory, blobs: usize) {
    for _ in 0..400 {
        if store.written().len() >= blobs {
            return;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }

    panic!(
        "the store should hold {blobs} blobs by now, it holds {:?}",
        store.written()
    );
}
