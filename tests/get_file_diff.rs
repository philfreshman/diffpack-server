//! `get_file_diff`, driven the way an agent drives it.
//!
//! One seam: the wire. `tools/list` for what a client is told and
//! `tools/call` for what it gets back, both through `router_with` over the
//! fixture archives. Testing here rather than at the handler is what keeps
//! the definition and the handler from drifting apart: a schema that stopped
//! describing what the handler reads is a confident wrong answer to a model,
//! and a handler test would go on passing through it.
//!
//! # Where an expected diff comes from
//!
//! The engine, wherever the engine can be asked. `get_diff_content` is
//! public, so the case where both versions have the file and they differ is
//! held against the engine's own output over the fixture's own bytes — which
//! is what makes the assertion able to disagree with this crate rather than
//! agree with it by construction.
//!
//! The other four cases cannot be asked for. `build_diff_result` is a private
//! `fn` in `diffpack-engine` 0.3.0 and `get_diff_for_path` is
//! `#[wasm_bindgen]`, so neither is callable from here and `src/engine.rs`
//! re-exports neither. Those four are held against literals worked out by
//! hand from the table in #15, which is the contract they exist to reproduce.
//!
//! # What is deliberately not asserted here
//!
//! Where a cut falls, what the marker says, and that the ceiling is measured
//! on serialised bytes rather than on length. Those are `src/page.rs`'s and
//! `tests/page.rs` holds them against generated text. What this suite asserts
//! is that this tool goes *through* that module rather than around it.
//!
//! That the handle format is what it is. `tests/handle.rs` holds the four
//! ways a handle is refused against hand-built payloads. What is here is two
//! of them reaching *this* tool's argument, because a tool that took a handle
//! and verified it late would pass every test in that file.
//!
//! Nor that no `description` an agent reads names a Rust path — that is a
//! rule every tool is held to rather than a fact about this one, so
//! `tests/tools.rs` holds it over the whole of `tools/list`.

use axum::body::Body;
use axum::http::Request;
use diffpack_server::engine;
use diffpack_server::handle::{DiffHandle, Inputs};
use diffpack_server::mcp::Diffpack;
use diffpack_server::registry::Registry;
use diffpack_server::router;
use diffpack_server::tools::Ctx;
use http_body_util::BodyExt;
use serde_json::{json, Value};
use tower::ServiceExt;

const CURRENT: &str = "2026-07-28";

const TOOL: &str = "get_file_diff";

/// The fixture sets this suite is served from, instead of the registries.
///
/// The root rather than one seam's directory inside it: `Ctx::fixture` gives
/// every seam a fixture adapter, so nothing this suite builds can reach a
/// registry — including a seam this tool does not use today.
const FIXTURES: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/fixtures");

// ---------------------------------------------------------------------------
// The bytes the fixtures hold
// ---------------------------------------------------------------------------
//
// Written out rather than read back out of the archive, so that an expected
// diff is computed from what `fixtures/archives/diffable-*.tgz` is known to
// contain rather than from whatever this crate's extractor returned. A test
// that took both sides from the extractor would agree with it about a file it
// had decoded wrongly.

/// `src/index.js` in `diffable` 1.0.0 — the file the second version edits.
const INDEX_FROM: &str = "export function greet(name) {\n  return \"Hello, \" + name;\n}\n";

/// The same file in 2.0.0.
const INDEX_TO: &str = "export function greet(name) {\n  return \"Hi, \" + name;\n}\n";

// ---------------------------------------------------------------------------
// The five cases
// ---------------------------------------------------------------------------

/// A file both versions have and that changed is the engine's own diff.
///
/// The one case the engine can be asked about directly: `get_diff_content` is
/// public, so the expectation is its output over the fixture's bytes rather
/// than a string written beside the implementation. `context_lines: "full"`
/// because that is the setting #15 defines as the engine's output untouched;
/// what a trimmed answer looks like is a question further down this file.
#[tokio::test]
async fn a_changed_file_is_the_engines_own_diff() {
    let answer = patch(json!({
        "handle": diffable(),
        "path": "src/index.js",
        "context_lines": "full",
    }))
    .await;

    assert_eq!(
        answer["text"],
        json!(engine::get_diff_content(
            "src/index.js",
            INDEX_FROM,
            INDEX_TO,
            false
        )),
        "a file both versions have and that changed is rendered by the engine \
         rather than by this crate, byte for byte: got {answer}"
    );
    assert_eq!(
        answer["isDiff"],
        json!(true),
        "a file that changed is a diff, which is what tells a viewer to render \
         it as one: got {answer}"
    );
}

/// A file only the second version has is every line of it, added.
///
/// A literal rather than the engine, for the reason in the header: the
/// renderer that produces this is private to `diffpack-engine` and there is
/// nothing to call. What is written out is the table in #15 — `/dev/null` on
/// the left, every line prefixed — down to the trailing `+ ` that the
/// engine's split leaves on a file ending in a newline.
#[tokio::test]
async fn a_file_only_the_second_version_has_is_every_line_added() {
    let answer = patch(json!({
        "handle": diffable(),
        "path": "src/added.js",
        "context_lines": "full",
    }))
    .await;

    assert_eq!(
        answer["text"],
        json!("--- /dev/null\n+++ to/src/added.js\n+ export const fresh = true;\n+ "),
        "a file the first version does not have is `/dev/null` against the \
         second version's, every line prefixed: got {answer}"
    );
    assert_eq!(
        answer["isDiff"],
        json!(true),
        "an added file is a diff — there is a `+` on every line of it: got {answer}"
    );
}

/// A file only the first version has is every line of it, removed.
///
/// The mirror of the case above, and the `path` argument reads the other way
/// round with it: there is nothing at this path in the second version, so
/// what was passed is where the file *was*.
#[tokio::test]
async fn a_file_only_the_first_version_has_is_every_line_removed() {
    let answer = patch(json!({
        "handle": diffable(),
        "path": "src/removed.js",
        "context_lines": "full",
    }))
    .await;

    assert_eq!(
        answer["text"],
        json!("--- from/src/removed.js\n+++ /dev/null\n- export const gone = true;\n- "),
        "a file the second version does not have is the first version's \
         against `/dev/null`, every line prefixed: got {answer}"
    );
    assert_eq!(
        answer["isDiff"],
        json!(true),
        "a removed file is a diff — there is a `-` on every line of it: got {answer}"
    );
}

/// A file both versions have byte for byte is the file, not a diff of it.
///
/// The case `isDiff` exists for. Rendering it as a patch of nothing but
/// context lines would be true and useless — an agent would parse a file as a
/// diff, strip a prefix off every line and read a file it had subtly altered.
/// So the file comes back as itself and the flag says to read it as one.
#[tokio::test]
async fn a_file_neither_version_touched_is_the_file_itself() {
    let answer = patch(json!({
        "handle": diffable(),
        "path": "README.md",
        "context_lines": "full",
    }))
    .await;

    assert_eq!(
        answer["text"],
        json!("# diffable\n"),
        "a file that did not change is its own content, with no header and no \
         prefix on any line: got {answer}"
    );
    assert_eq!(
        answer["isDiff"],
        json!(false),
        "there is no diff to read here, which is what tells a caller to render \
         this as a file: got {answer}"
    );
}

/// A path neither version has is a sentence saying so, not a failure.
///
/// The engine's fifth case, and it is an answer rather than an error on
/// purpose: a comparison is a pair of versions and "this path is in neither
/// of them" is a fact about the pair. What a caller does next is look at the
/// tree, which is what the sentence is for.
#[tokio::test]
async fn a_path_in_neither_version_says_so_rather_than_failing() {
    let answer = patch(json!({
        "handle": diffable(),
        "path": "src/never-was.js",
        "context_lines": "full",
    }))
    .await;

    assert_eq!(
        answer["text"],
        json!("File not present in either version."),
        "a path in neither version is the engine's own sentence, word for \
         word: got {answer}"
    );
    assert_eq!(
        answer["isDiff"],
        json!(false),
        "a sentence is not a patch, and a caller that parsed it as one would \
         read a hunk out of prose: got {answer}"
    );
}

// ---------------------------------------------------------------------------
// Getting there
// ---------------------------------------------------------------------------

/// A handle for `diffable` 1.0.0 → 2.0.0, minted rather than fetched.
///
/// The tool that mints one is called where the *agreement* between the two is
/// what is being asserted. Everywhere else a handle is just the argument, and
/// minting it here is one call rather than two — the shape
/// `tests/get_diff_tree.rs` settled on.
fn diffable() -> String {
    handle("diffable", "1.0.0", "2.0.0", false)
}

/// A handle for one npm comparison, at the defaults `diff_package_versions`
/// would have used unless `ignore_whitespace` says otherwise.
fn handle(package: &str, from: &str, to: &str, ignore_whitespace: bool) -> String {
    DiffHandle::mint(Inputs {
        registry: Registry::Npm,
        package: package.to_owned(),
        from_version: from.to_owned(),
        to_version: to.to_owned(),
        similarity_threshold: 0.75,
        ignore_whitespace,
    })
    .encode()
}

/// The answer this tool gives to `arguments`, or a panic naming the failure.
///
/// The structured half, which is where a patch's `text`, `isDiff` and the
/// three fields describing a cut arrive.
async fn patch(arguments: Value) -> Value {
    let result = call(arguments).await;

    assert_ne!(
        result["isError"],
        json!(true),
        "expected a patch, got a tool error: {result}"
    );

    result["structuredContent"].clone()
}

/// Call this tool with `arguments`, returning the `result` — or panicking with
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

/// The same request, with the body the client received beside the answer.
///
/// The bytes rather than the structure, because that is what the response
/// ceiling bounds: what Vercel refuses is the frame this server wrote, and
/// two frames that parse alike can still differ in size.
async fn respond(body: Value) -> (String, Value) {
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

    (String::from_utf8_lossy(&bytes).into_owned(), answer)
}
