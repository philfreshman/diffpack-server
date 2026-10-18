mod common;

use std::collections::HashMap;

use common::Client;
use diffpack_server::archive::Archive;
use diffpack_server::registry::Registry;
use serde_json::{json, Value};

const TOOL: &str = "resolve_archive_url";

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

async fn call(arguments: Value) -> Value {
    Client::fixture().call(TOOL, arguments).await
}

fn fixture_index() -> HashMap<String, Option<String>> {
    let index = std::fs::read(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/fixtures/archives/index.json"
    ))
    .expect("the fixture index is checked in");

    serde_json::from_slice(&index).expect("the fixture index is JSON")
}
