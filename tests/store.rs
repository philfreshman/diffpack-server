//! Cached diff results, driven the way an agent drives them.
//!
//! The cache has no surface of its own: nothing calls it, nothing reads it
//! back, and the only thing an agent ever sees of it is that the same
//! comparison asked for twice was cheap the second time. So every test here
//! goes over the wire — `tools/call` on `diff_package_versions`, through the
//! service factory `router_with` takes — and the store under it is an
//! in-process one the test holds a handle to, the way `tests/log.rs` holds a
//! [`Capture`](diffpack_server::log::Capture).
//!
//! That handle is what lets a test say more than "it was cheap": it names the
//! blobs an entry is, and when each of them was written. Both are the
//! cache's contract rather than its internals — the pathnames are what #27
//! reads a result back from, and the upload moment is the order #22 evicts
//! in.
//!
//! # What a test here is not
//!
//! A test of the Vercel Blob client. That is #20's, in `src/store/blob.rs`,
//! against a stub HTTP server, because the client is private to that module.
//! What is asserted here is the policy above it — which blobs an entry is,
//! when they are written, what happens when they are too big, and what
//! happens when the store is not there — and that policy is the same code
//! whichever adapter is underneath.

use axum::body::Body;
use axum::http::Request;
use diffpack_server::mcp::Diffpack;
use diffpack_server::router;
use diffpack_server::store::Memory;
use diffpack_server::tools::Ctx;
use http_body_util::BodyExt;
use serde_json::{json, Value};
use tower::ServiceExt;

const CURRENT: &str = "2026-07-28";

const TOOL: &str = "diff_package_versions";

/// The fixture sets this suite is served from, instead of the registries.
const FIXTURES: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/fixtures");

/// The pair every test here compares unless it needs another.
///
/// `diffable` 1.0.0 → 2.0.0 is the same pair `tests/diff_package_versions.rs`
/// works its totals out from: one file of each status, and small enough that
/// nothing here is near a size cap by accident.
fn diffable() -> Value {
    json!({
        "registry": "npm",
        "package": "diffable",
        "from_version": "1.0.0",
        "to_version": "2.0.0",
    })
}

/// The whole point of the cache, as an agent sees it.
///
/// The first call downloads two archives and compares them; the second is
/// handed what the first worked out. `cached` is how an agent tells the two
/// apart, and it is the only part of this an agent ever sees.
#[tokio::test]
async fn the_same_diff_asked_for_twice_is_remembered_the_second_time() {
    let store = Memory::new();

    let first = call(&store, diffable()).await;
    assert_eq!(
        first["structuredContent"]["cached"],
        json!(false),
        "nothing had been diffed yet, got {first}"
    );

    let second = call(&store, diffable()).await;
    assert_eq!(
        second["structuredContent"]["cached"],
        json!(true),
        "the same comparison a second time is the one that was written, got \
         {second}"
    );
}

/// Remembering an answer is not allowed to change it.
///
/// A cache whose answers differ from the computation's is worse than no
/// cache: the difference is invisible to whoever asked and shows up later as
/// two agents disagreeing about the same comparison. So the whole answer is
/// compared and not a field of it — `cached` is the one field that is
/// *supposed* to differ, which is why it is the one taken out first.
///
/// What makes this hold is that what is remembered is the tree rather than
/// the answer: the totals and the sample are walked out of it on both paths,
/// so there is no second writer for them to disagree with.
#[tokio::test]
async fn a_remembered_answer_is_the_one_that_was_computed() {
    let store = Memory::new();

    let computed = call(&store, diffable()).await;
    let remembered = call(&store, diffable()).await;

    assert_eq!(
        but_for_cached(remembered),
        but_for_cached(computed),
        "everything but `cached` is the same answer, the text block included"
    );
}

/// An entry is two blobs, under the name the comparison has.
///
/// The pathnames are the contract rather than an implementation detail: #27
/// computes the same `diff_id` in TypeScript and reads
/// `diffs/v1/{diff_id}/meta.json` with no server in the path, so a layout
/// this suite does not pin is a layout that can move without anything here
/// noticing.
///
/// Both of them, because half an entry is not a cache hit. A `meta.json`
/// written without its `patches.json` is a result whose patches are missing
/// with nothing saying so.
#[tokio::test]
async fn a_remembered_diff_is_two_blobs_under_the_name_the_comparison_has() {
    let store = Memory::new();

    let answer = call(&store, diffable()).await;
    let diff_id = answer["structuredContent"]["diff_id"]
        .as_str()
        .unwrap_or_else(|| panic!("the answer names the comparison, got {answer}"));

    assert_eq!(
        store.written(),
        vec![
            format!("diffs/v1/{diff_id}/meta.json"),
            format!("diffs/v1/{diff_id}/patches.json"),
        ],
        "one comparison is these two blobs and no others"
    );
}

/// What `patches.json` is for: every changed file, rendered once.
///
/// This is the whole reason the patches are written at all. Both archives are
/// extracted at this moment, so rendering every changed file costs almost
/// nothing; asking for one later costs two downloads (#15).
///
/// The expected patches are worked out from the fixture and from the
/// renderer's contract, not from running it — four shapes, one per case the
/// pair reaches:
///
/// - a file that is only in the second version is `/dev/null` against it,
///   every line prefixed `+`, and the trailing newline makes a last empty one
/// - a file that is only in the first is the mirror of that
/// - a file in both, changed, is the engine's unified diff
/// - a file in both whose content is identical is that content and
///   `is_diff: false`, so that a reader renders a file as a file — which is
///   what a rename with an unchanged body is
///
/// `README.md` is in neither version's patch set, because it did not change.
/// An unchanged file's patch is the file, and a cache that stored one would
/// be storing the package.
#[tokio::test]
async fn every_changed_file_is_remembered_with_its_patch_already_rendered() {
    let store = Memory::new();

    let answer = call(&store, diffable()).await;
    let diff_id = answer["structuredContent"]["diff_id"]
        .as_str()
        .unwrap_or_else(|| panic!("the answer names the comparison, got {answer}"));

    assert_eq!(
        blob(&store, &format!("diffs/v1/{diff_id}/patches.json")),
        json!({
            "src/added.js": {
                "data": "--- /dev/null\n+++ to/src/added.js\n+ export const fresh = true;\n+ ",
                "is_diff": true,
            },
            "src/index.js": {
                "data": "--- from/src/index.js\n+++ to/src/index.js\n  \
                         export function greet(name) {\n-   return \"Hello, \" \
                         + name;\n+   return \"Hi, \" + name;\n  }",
                "is_diff": true,
            },
            "src/new-name.js": {
                "data": "export const stable = 1;\nexport const alsoStable = 2;\n",
                "is_diff": false,
            },
            "src/removed.js": {
                "data": "--- from/src/removed.js\n+++ /dev/null\n- export const gone = true;\n- ",
                "is_diff": true,
            },
        }),
        "one patch per changed file and none for the one that did not change"
    );
}

/// The blob at `pathname`, as the JSON it is.
fn blob(store: &Memory, pathname: &str) -> Value {
    let bytes = store
        .blob(pathname)
        .unwrap_or_else(|| panic!("`{pathname}` should be there, {:?} is", store.written()));

    serde_json::from_slice(&bytes)
        .unwrap_or_else(|e| panic!("`{pathname}` should be JSON, got {e}"))
}

/// One answer with `cached` taken out of both copies of it.
///
/// `tools::invoke` puts a tool's answer on the wire twice — as
/// `structuredContent` and as the text block `rmcp` mirrors it into — so a
/// field dropped from one is still in the other. The mirror is read back as
/// JSON rather than compared as a string, which is also what makes the
/// comparison above about the answer rather than about how it was spelled.
fn but_for_cached(mut answer: Value) -> Value {
    let mirrored = answer["content"][0]["text"]
        .as_str()
        .unwrap_or_else(|| panic!("a text block, got {answer}"))
        .to_owned();
    answer["content"][0]["text"] =
        serde_json::from_str(&mirrored).unwrap_or_else(|e| panic!("the mirror is JSON: {e}"));

    for at in ["/structuredContent", "/content/0/text"] {
        let copy = answer
            .pointer_mut(at)
            .and_then(Value::as_object_mut)
            .unwrap_or_else(|| panic!("an object at `{at}` to take `cached` out of"));

        assert!(
            copy.remove("cached")
                .is_some_and(|cached| cached.is_boolean()),
            "every copy of an answer says whether it was cached, `{at}` does not"
        );
    }

    answer
}

// ---------------------------------------------------------------------------
// Driving the wire
// ---------------------------------------------------------------------------

/// Call `diff_package_versions` with `arguments`, against a server whose
/// cache is `store`.
///
/// Panics with the JSON-RPC error rather than returning it: nothing in this
/// suite asks a question whose answer is a protocol error, so one arriving
/// means the call was built wrongly.
async fn call(store: &Memory, arguments: Value) -> Value {
    let answer = post(
        store,
        json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "tools/call",
            "params": {
                "name": TOOL,
                "arguments": arguments,
                "_meta": {
                    "io.modelcontextprotocol/protocolVersion": CURRENT,
                    "io.modelcontextprotocol/clientCapabilities": {},
                },
            },
        }),
    )
    .await;

    if let Some(error) = answer.get("error") {
        panic!("expected a result, got JSON-RPC error {error}");
    }
    answer["result"].clone()
}

/// A request as a conforming `2026-07-28` client sends it, to a server whose
/// archives come from `fixtures/` and whose cache is `store`.
///
/// The context is built the way production builds one — through the service
/// factory — and then has its store replaced, which is the same `..self`
/// spread `Ctx::logging_to` is and not a builder that fills the seams it was
/// not given. Every other seam is still the fixture set's.
async fn post(store: &Memory, body: Value) -> Value {
    let request = Request::builder()
        .method("POST")
        .uri("/mcp")
        .header("host", "mcp.diffpack.io")
        .header("accept", "application/json, text/event-stream")
        .header("content-type", "application/json")
        .header("mcp-protocol-version", CURRENT)
        .header("mcp-method", "tools/call")
        .header("mcp-name", TOOL)
        .body(Body::from(body.to_string()))
        .expect("the request should build");

    let ctx = Ctx::fixture(FIXTURES).storing_in(store.store());
    let router = router::router_with(move || Ok(Diffpack::with_ctx(ctx.clone())), Vec::new());

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
