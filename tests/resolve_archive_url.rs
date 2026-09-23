//! `resolve_archive_url`, driven the way an agent drives it.
//!
//! The same seam the other tool suites use, for the same reason: everything
//! here goes over the wire — `tools/call` for what a client gets back —
//! because a test that called the handler would keep passing while the
//! definition beside it stopped matching.
//!
//! What is deliberately not here: the rules every tool is held to. A
//! description that says something, a declared output shape, the read-only
//! and open-world hints, the registry enum — those are facts about the
//! collection and `tests/tools.rs` asserts them once, over all eight. What is
//! left is this tool's own: the URL patterns three registries serve archives
//! at, and the one registry that publishes no such pattern at all.

mod common;

use std::collections::HashMap;

use common::Client;
use diffpack_server::archive::Archive;
use diffpack_server::registry::Registry;
use serde_json::{json, Value};

const TOOL: &str = "resolve_archive_url";

// ---------------------------------------------------------------------------
// What a client is told
// ---------------------------------------------------------------------------

/// The three arguments this tool resolves from, where an agent reads them.
#[tokio::test]
async fn the_definition_carries_everything_an_agent_needs() {
    let tool = Client::fixture().listed(TOOL).await;

    for field in ["registry", "package", "version"] {
        assert!(
            tool["inputSchema"]["properties"][field].is_object(),
            "the input schema should describe `{field}`, got {}",
            tool["inputSchema"]
        );
    }

    assert_eq!(
        tool["annotations"]["idempotentHint"], true,
        "the same three arguments always resolve to the same URL, got {}",
        tool["annotations"]
    );
}

// ---------------------------------------------------------------------------
// What it answers
// ---------------------------------------------------------------------------

/// npm serves a version's tarball at a path anyone can build, which is why
/// this tool can answer without asking npm anything. The expected URL is from
/// #10, not from running the code.
#[tokio::test]
async fn npm_resolves_to_the_registry_tarball() {
    let result = call(json!({
        "registry": "npm",
        "package": "zod",
        "version": "4.0.0",
    }))
    .await;

    assert_eq!(
        result["structuredContent"]["url"],
        "https://registry.npmjs.org/zod/-/zod-4.0.0.tgz"
    );
    assert_eq!(
        result["isError"],
        json!(false),
        "a resolved URL is not an error, got {result}"
    );
}

/// The one npm rule that is not obvious: the path keeps the scope and the
/// filename drops it, so `@types/node` is served from `node-20.1.0.tgz`. A
/// tool that got this wrong would send every scoped package's diff to a 404.
#[tokio::test]
async fn a_scoped_npm_package_drops_its_scope_from_the_filename() {
    let result = call(json!({
        "registry": "npm",
        "package": "@types/node",
        "version": "20.1.0",
    }))
    .await;

    assert_eq!(
        result["structuredContent"]["url"],
        "https://registry.npmjs.org/@types/node/-/node-20.1.0.tgz"
    );
}

/// A version is spelled the way it arrived, `v` and all: `v4.0.0` and
/// `4.0.0` are different versions, and the URL is the registry's answer to
/// which one exists. A tool that tidied the `v` away would resolve a
/// version nobody asked for. The expected URL is npm's pattern with the
/// version pasted in as written, not the output of running the code.
#[tokio::test]
async fn a_version_reaches_the_registry_exactly_as_it_was_written() {
    let result = call(json!({
        "registry": "npm",
        "package": "zod",
        "version": "v4.0.0",
    }))
    .await;

    assert_eq!(
        result["structuredContent"]["url"],
        "https://registry.npmjs.org/zod/-/zod-v4.0.0.tgz"
    );
}

/// crates.io serves from the static host rather than the API one.
#[tokio::test]
async fn crates_io_resolves_to_the_static_host() {
    let result = call(json!({
        "registry": "crates",
        "package": "serde",
        "version": "1.0.0",
    }))
    .await;

    assert_eq!(
        result["structuredContent"]["url"],
        "https://static.crates.io/crates/serde/serde-1.0.0.crate"
    );
}

/// The structured answer is the contract, and the text beside it is what a
/// client without structured-content support renders. Both have to be there:
/// one of them is what the model reads.
#[tokio::test]
async fn the_answer_is_structured_and_also_readable() {
    let result = call(json!({
        "registry": "crates",
        "package": "serde",
        "version": "1.0.0",
    }))
    .await;

    assert!(result["structuredContent"].is_object(), "got {result}");

    let text = result["content"][0]["text"]
        .as_str()
        .expect("a result should carry a text block too");
    assert!(
        text.contains("static.crates.io"),
        "the text block should carry the answer, got {text}"
    );
}

/// What this tool is *for* is letting an agent show where a diff's bytes came
/// from, which is only true if the URL it answers with is the URL the fetch
/// path asks for. The two agree by asking `registry` the same question rather
/// than by holding two copies of a pattern, and this is where that is
/// asserted instead of assumed.
///
/// `src/archive/`'s fixtures are keyed by URL, so the fetch below succeeds
/// only if the path it took asked for a URL in that set — and the answer this
/// tool gave is a key in it. A tool that built its own URL, or a fetch path
/// that did, parts company here.
#[tokio::test]
async fn the_url_this_tool_answers_with_is_the_one_the_fetch_path_asks_for() {
    let result = call(json!({
        "registry": "npm",
        "package": "@types/node",
        "version": "20.1.0",
    }))
    .await;
    let answered = result["structuredContent"]["url"]
        .as_str()
        .expect("the tool answers with a URL")
        .to_owned();

    let files = Archive::fixture(concat!(env!("CARGO_MANIFEST_DIR"), "/fixtures/archives"))
        .fetch(Registry::Npm, "@types/node", "20.1.0")
        .await
        .expect("the fetch path asks for an archive the fixture set has");

    assert!(
        !files.paths().is_empty(),
        "the fetch path came back with a version's files"
    );
    assert!(
        fixture_index().contains_key(&answered),
        "the answered URL should be the one the fetch path was served from, got {answered}"
    );
}

// ---------------------------------------------------------------------------
// How it fails
// ---------------------------------------------------------------------------

/// A registry this tool cannot resolve is something the model can act on —
/// by asking for a registry it can — so it goes on the channel the model
/// reads. PyPI is the real case: its archive URL is listed only in its own
/// metadata, so there is nothing to build from a package name and a version.
#[tokio::test]
async fn a_registry_this_tool_cannot_resolve_is_a_tool_error() {
    let result = call(json!({
        "registry": "pypi",
        "package": "requests",
        "version": "2.31.0",
    }))
    .await;

    assert_eq!(
        result["isError"],
        json!(true),
        "the model is the one who can pick another registry, got {result}"
    );

    let text = result["content"][0]["text"]
        .as_str()
        .expect("a tool error carries text for the model");
    assert!(
        text.contains("pypi"),
        "the message should name what was asked for, got {text}"
    );
    for resolvable in ["npm", "crates"] {
        assert!(
            text.contains(resolvable),
            "a message the model can act on names `{resolvable}`, which would have \
             worked: got {text}"
        );
    }
}

/// Arguments that do not validate are the client's mistake, not the model's,
/// so they take the protocol channel and `-32602`. The model never sees this
/// one; the client is expected to correct the call it sent.
#[tokio::test]
async fn arguments_that_do_not_validate_are_a_protocol_error() {
    let answer = Client::fixture()
        .post(json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "tools/call",
            "params": {
                "name": TOOL,
                "arguments": { "registry": "npm", "package": "zod" },
            },
        }))
        .await;

    assert_eq!(
        answer["error"]["code"], -32602,
        "a missing argument is invalid params, got {answer}"
    );
    assert!(
        answer["result"]["isError"].is_null(),
        "a call that never ran is not a tool that failed, got {answer}"
    );
}

/// A registry that is not one of the three does not reach a handler: the
/// schema declares the enum, so the value fails to validate and the client —
/// which was told the list — is who can fix the call. The refusal names the
/// registries that exist, because a client told only that `go` is wrong has
/// to go and find out what is right.
#[tokio::test]
async fn a_registry_outside_the_enum_is_refused_by_naming_the_ones_that_exist() {
    let answer = Client::fixture()
        .post(json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "tools/call",
            "params": {
                "name": TOOL,
                "arguments": { "registry": "go", "package": "logrus", "version": "1.9.3" },
            },
        }))
        .await;

    assert_eq!(
        answer["error"]["code"], -32602,
        "an argument outside the declared enum is invalid params, got {answer}"
    );

    let message = answer["error"]["message"]
        .as_str()
        .expect("a protocol error carries a message");
    for known in ["npm", "crates", "pypi"] {
        assert!(
            message.contains(known),
            "the refusal should name `{known}`, got {message}"
        );
    }
}

// ---------------------------------------------------------------------------
// Driving the endpoint
// ---------------------------------------------------------------------------

/// Call this tool with `arguments`, returning the `result`.
async fn call(arguments: Value) -> Value {
    Client::fixture().call(TOOL, arguments).await
}

/// The archives the fetch path is served from, by the URL each stands in for.
///
/// A `null` value is a URL the fixture set says serves nothing, which is
/// still a URL the set knows about — what this test asks is whether the
/// answered URL is one the fetch path would have been served from at all.
fn fixture_index() -> HashMap<String, Option<String>> {
    let index = std::fs::read(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/fixtures/archives/index.json"
    ))
    .expect("the fixture index is checked in");

    serde_json::from_slice(&index).expect("the fixture index is JSON")
}
