//! `get_file_content`, driven the way an agent drives it.
//!
//! Two seams, the same two `tests/list_package_files.rs` uses and for the
//! same reason. Most of what is here goes over the wire — `tools/list` for
//! what a client is told, `tools/call` for what it gets back — because a test
//! that only called the handler would keep passing while the definition
//! beside it stopped matching, which is the failure #41 exists to prevent.
//! The handler is reached directly only where the question is about the
//! answer's *type* rather than about its JSON.
//!
//! What is deliberately not re-proven here: where a cut falls, what the
//! marker says, and that the ceiling is measured on serialised bytes rather
//! than on length. Those are `src/page.rs`'s and `tests/page.rs` holds them
//! against generated text, which is a stronger fixture than any file in a
//! package. What this suite asserts is that this tool goes *through* that
//! module rather than around it.
//!
//! Nor that no `description` an agent reads names a Rust path — that is a
//! rule every tool is held to rather than a fact about this one, so
//! `tests/tools.rs` holds it over the whole of `tools/list`.
//!
//! Extraction, and the lossy decoding a file that is not UTF-8 goes through,
//! are `tests/archive.rs`'s. The archives below are the fixture set, so a
//! fetch that built a URL of its own finds nothing.

use axum::body::Body;
use axum::http::Request;
use diffpack_server::archive::Archive;
use diffpack_server::mcp::Diffpack;
use diffpack_server::router;
use diffpack_server::tools::Ctx;
use http_body_util::BodyExt;
use serde_json::{json, Value};
use tower::ServiceExt;

const CURRENT: &str = "2026-07-28";

const TOOL: &str = "get_file_content";

/// The archives this suite is served from, instead of the registries.
const FIXTURES: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/fixtures/archives");

// ---------------------------------------------------------------------------
// What it answers
// ---------------------------------------------------------------------------

/// The tracer bullet: one file out of one npm package, exactly as it was
/// packed.
///
/// The expected text is the fixture's own, written by hand and packed by
/// `tar` when `scripts/make-archive-fixtures.sh` was written — not read back
/// out of this server, which would agree with a bug.
///
/// The path is `package.json` rather than `package/package.json`: the
/// archive's top-level directory is stripped before a path is ever asked
/// for, which is the one thing about a path an agent cannot infer.
#[tokio::test]
async fn a_file_from_an_npm_package_comes_back_with_its_exact_content() {
    let result = call(json!({
        "registry": "npm",
        "package": "@types/node",
        "version": "20.1.0",
        "path": "package.json",
    }))
    .await;

    assert_eq!(
        result["isError"],
        json!(false),
        "a file the fixture set has is not an error, got {result}"
    );
    assert_eq!(
        result["structuredContent"]["text"],
        "{\n  \"name\": \"@types/node\",\n  \"version\": \"20.1.0\"\n}\n",
        "got {result}"
    );
}

/// A crate's file, out of a different wrapper and a different extension and
/// through the same path.
///
/// Green the moment the npm one was, because the fetch path is the same code
/// for all three registries — which is the thing being asserted. A tool that
/// had learned anything registry-shaped on its way to a file would be the
/// one to fail here.
#[tokio::test]
async fn a_file_from_a_crate_comes_back_with_its_exact_content() {
    let result = call(json!({
        "registry": "crates",
        "package": "serde",
        "version": "1.0.0",
        "path": "src/lib.rs",
    }))
    .await;

    assert_eq!(
        result["structuredContent"]["text"], "pub fn serialize() {}\n",
        "got {result}"
    );
}

/// PyPI is two requests rather than one — the version's metadata, then the
/// artefact it names — and this tool makes neither of them. That a file
/// comes back at all is the assertion: the second hop belongs to the module
/// that fetches, and a tool that had to know PyPI needs asking would be
/// carrying that module's job around.
///
/// `setup.py` is in the source distribution and not in the wheel, so which
/// artefact was chosen is visible in the text rather than only in a URL
/// nobody sees.
#[tokio::test]
async fn a_file_from_a_pypi_package_comes_back_through_the_metadata_hop() {
    let result = call(json!({
        "registry": "pypi",
        "package": "requests",
        "version": "2.31.0",
        "path": "setup.py",
    }))
    .await;

    assert_eq!(
        result["structuredContent"]["text"],
        "from setuptools import setup\n\nsetup(name=\"requests\", version=\"2.31.0\")\n",
        "got {result}"
    );
}

// ---------------------------------------------------------------------------
// Cutting it
// ---------------------------------------------------------------------------

/// A cut has to be loud. A silently shortened file is how an agent concludes
/// a function does not exist — it read what it was given, found no
/// `serialize`, and had nothing in the answer to tell it the file went on.
///
/// So all three are asserted together: the text that came back, that it says
/// it was cut, and that the byte count is the *file's* and not the excerpt's.
/// A tool reporting the returned length there would be telling an agent that
/// a file it has seen a fifth of is a fifth long.
///
/// 51 is the fixture's own size, from `wc -c` on the archive `tar` packed —
/// not from this server, which would agree with a bug.
///
/// Where the cut falls and what the marker says are `src/page.rs`'s, and
/// `tests/page.rs` holds them. What is asserted here is that this tool goes
/// through that module: it keeps what was asked for, and it does not invent
/// a count of its own.
#[tokio::test]
async fn a_file_over_max_bytes_is_cut_and_says_so_and_states_the_real_size() {
    let result = call(json!({
        "registry": "npm",
        "package": "@types/node",
        "version": "20.1.0",
        "path": "package.json",
        "max_bytes": 12,
    }))
    .await;

    let text = result["structuredContent"]["text"]
        .as_str()
        .unwrap_or_else(|| panic!("the answer carries the text, got {result}"));

    assert!(
        text.starts_with("{\n  \"name\": "),
        "the first 12 bytes of the file, and the cut falls after them: got {text:?}"
    );
    assert_eq!(
        result["structuredContent"]["truncated"],
        json!(true),
        "a cut an agent cannot see is how it concludes a function is missing, got {result}"
    );
    assert_eq!(
        result["structuredContent"]["bytes"],
        json!(51),
        "the whole file's size, not the excerpt's, got {result}"
    );
}

/// A file that fits comes back whole and says so, which is the half that
/// stops `truncated` from being decoration: an agent that saw it set on
/// every answer would learn to ignore it.
#[tokio::test]
async fn a_file_that_fits_comes_back_whole_and_says_it_was_not_cut() {
    let result = call(json!({
        "registry": "npm",
        "package": "@types/node",
        "version": "20.1.0",
        "path": "package.json",
    }))
    .await;

    assert_eq!(
        result["structuredContent"]["truncated"],
        json!(false),
        "51 bytes is not over any cap this server has, got {result}"
    );
    assert_eq!(
        result["structuredContent"]["bytes"],
        json!(51),
        "the size is stated whether or not there was a cut, got {result}"
    );
}

// ---------------------------------------------------------------------------
// Text that was never text
// ---------------------------------------------------------------------------

/// A file that is not valid UTF-8 comes back decoded lossily, which is what
/// the extractor already does to it, and the answer says so.
///
/// Not an error, because a binary file in a package is an ordinary thing and
/// an agent asking about one has not made a mistake. But without the flag it
/// is handed a string of replacement characters and cannot tell a PNG from a
/// source file somebody saved in the wrong encoding — and those have
/// different next moves.
///
/// The expected text is what `python3 -c` said the fixture's ten bytes decode
/// to, not what this server said: a PNG signature whose first byte stands
/// alone, then two bytes that are never valid. Three replacement characters,
/// and `PNG` still legible between them.
#[tokio::test]
async fn a_file_that_is_not_utf8_comes_back_lossily_decoded_and_flagged() {
    let result = call(json!({
        "registry": "npm",
        "package": "odd-files",
        "version": "1.0.0",
        "path": "logo.png",
    }))
    .await;

    assert_eq!(
        result["isError"],
        json!(false),
        "a package shipping a binary file is ordinary, got {result}"
    );
    assert_eq!(
        result["structuredContent"]["validUtf8"],
        json!(false),
        "without this an agent cannot tell a binary from a mis-encoded source \
         file, and the two have different next moves: got {result}"
    );
    assert_eq!(
        result["structuredContent"]["text"], "\u{FFFD}PNG\r\n\u{1a}\n\u{FFFD}\u{FFFD}",
        "got {result}"
    );
}

/// The other half, and the one that keeps the flag from being decoration: an
/// ordinary source file says it decoded cleanly. A flag set on every answer
/// is a flag an agent learns to skip.
#[tokio::test]
async fn an_ordinary_file_says_it_is_valid_utf8() {
    let result = call(json!({
        "registry": "crates",
        "package": "serde",
        "version": "1.0.0",
        "path": "src/lib.rs",
    }))
    .await;

    assert_eq!(
        result["structuredContent"]["validUtf8"],
        json!(true),
        "got {result}"
    );
}

// ---------------------------------------------------------------------------
// How it fails
// ---------------------------------------------------------------------------

/// A directory has no content, and saying so is the whole point: the
/// extractor gives a directory the empty string, so a tool that passed that
/// through would tell an agent that `src` is a file with nothing in it. The
/// agent's next move — conclude the package ships an empty module — is wrong
/// and it has nothing to notice it with.
///
/// It is a tool error rather than a protocol one because the model is who
/// can fix it, by asking for a file inside the directory instead.
#[tokio::test]
async fn a_directory_is_a_tool_error_rather_than_an_empty_file() {
    let result = call(json!({
        "registry": "crates",
        "package": "serde",
        "version": "1.0.0",
        "path": "src",
    }))
    .await;

    assert_eq!(
        result["isError"],
        json!(true),
        "`src` is a directory of this crate's, and an empty string would read \
         as an empty file: got {result}"
    );

    let text = result["content"][0]["text"]
        .as_str()
        .expect("a tool error carries text for the model");
    assert!(
        text.contains("src") && text.contains("directory"),
        "the message should name the path and say what it is, since the \
         remedy is to ask for a file inside it: got {text}"
    );
}

/// A path the version does not have is something the model can act on — by
/// listing the version's files and asking for one of those — so it takes the
/// channel the model reads, and it names what was not found. A model told
/// only that a request failed has no next call to make.
#[tokio::test]
async fn a_path_the_version_does_not_have_is_a_tool_error_naming_it() {
    let result = call(json!({
        "registry": "crates",
        "package": "serde",
        "version": "1.0.0",
        "path": "src/nowhere.rs",
    }))
    .await;

    assert_eq!(
        result["isError"],
        json!(true),
        "the model is the one who can ask for a path that exists, got {result}"
    );

    let text = result["content"][0]["text"]
        .as_str()
        .expect("a tool error carries text for the model");
    for named in ["src/nowhere.rs", "serde", "1.0.0"] {
        assert!(
            text.contains(named),
            "the message should name `{named}`, which is what was asked for: got {text}"
        );
    }
}

// ---------------------------------------------------------------------------
// Driving the endpoint
// ---------------------------------------------------------------------------

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
/// archives come from `fixtures/archives/` rather than from the registries.
///
/// The fixture adapter is reached the way #39 says a tool's state is reached:
/// through the service factory `router_with` takes, which builds the [`Ctx`]
/// every handler is handed. A test that reached around it would be testing a
/// path production does not take.
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
            Ok(Diffpack::with_ctx(Ctx::with_archive(Archive::fixture(
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
