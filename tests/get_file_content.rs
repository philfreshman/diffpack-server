mod common;

use common::{Client, FIXTURES};
use diffpack_server::error::Failure;
use diffpack_server::page::{self, Excerpt};
use diffpack_server::registry::{PackageName, Registry, VersionName};
use diffpack_server::tools::get_file_content::{Args, GetFileContent};
use diffpack_server::tools::Tool;
use diffpack_server::tools::{Call, Ctx};
use serde_json::{json, Value};

const TOOL: &str = "get_file_content";

#[tokio::test]
async fn the_definition_carries_everything_an_agent_needs() {
    let tool = listed(TOOL).await;

    for field in ["registry", "package", "version", "path", "max_bytes"] {
        assert!(
            tool["inputSchema"]["properties"][field].is_object(),
            "the input schema should describe `{field}`, got {}",
            tool["inputSchema"]
        );
    }
    for required in ["registry", "package", "version", "path"] {
        assert!(
            tool["inputSchema"]["required"]
                .as_array()
                .is_some_and(|fields| fields.iter().any(|field| field == required)),
            "`{required}` is not optional, got {}",
            tool["inputSchema"]
        );
    }
    assert!(
        tool["inputSchema"]["required"]
            .as_array()
            .is_some_and(|fields| !fields.iter().any(|field| field == "max_bytes")),
        "a cap the server applies anyway is not something a caller has to \
         supply, got {}",
        tool["inputSchema"]
    );
    for field in ["text", "truncated", "bytes", "validUtf8"] {
        assert!(
            tool["outputSchema"]["properties"][field].is_object(),
            "the output schema should describe `{field}`, got {}",
            tool["outputSchema"]
        );
    }
    assert_eq!(
        tool["annotations"]["idempotentHint"], true,
        "a published version's contents are immutable, got {}",
        tool["annotations"]
    );
}

#[tokio::test]
async fn the_description_says_what_an_agent_would_otherwise_get_wrong() {
    let tool = listed(TOOL).await;
    let description = tool["description"].as_str().unwrap_or_else(|| {
        panic!("a tool an agent picks without documentation has one, got {tool}")
    });

    assert!(
        description.contains("src/index.js") && description.contains("zod-4.0.0/src/index.js"),
        "the description should show the shape of a path rather than describe \
         it, since the agent's mistake is a guessed path: got {description}"
    );
    assert!(
        description.contains("director"),
        "a directory is refused rather than answered with an empty string, \
         and an agent that does not know reads the refusal as a bug: got {description}"
    );
    assert!(
        description.contains("U+FFFD") || description.contains("replacement"),
        "a binary file comes back decoded rather than refused, which reads as \
         a corrupt file to an agent that was not told: got {description}"
    );
}

#[tokio::test]
async fn the_cap_argument_documents_the_number_that_binds() {
    let tool = listed(TOOL).await;
    let max_bytes = &tool["inputSchema"]["properties"]["max_bytes"];

    assert_eq!(
        max_bytes["maximum"],
        json!(page::PAYLOAD_CEILING),
        "got {max_bytes}"
    );
    assert_eq!(max_bytes["minimum"], json!(1), "got {max_bytes}");
    assert!(
        max_bytes["description"]
            .as_str()
            .is_some_and(|said| said.contains("narrowed")),
        "the description is the one the response module writes, and a doc \
         comment here would silently replace it: got {max_bytes}"
    );
}

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

#[tokio::test]
async fn a_file_larger_than_a_response_is_cut_without_being_asked() {
    let result = call(json!({
        "registry": "npm",
        "package": "odd-files",
        "version": "1.0.0",
        "path": "big.txt",
    }))
    .await;

    assert_eq!(
        result["structuredContent"]["truncated"],
        json!(true),
        "two megabytes do not fit in a four-and-a-half megabyte response \
         once the answer is carried twice, got {}",
        result["structuredContent"]["bytes"]
    );
    assert_eq!(
        result["structuredContent"]["bytes"],
        json!(2_000_000),
        "the file's whole size, so an agent knows how much it has not seen"
    );

    let shown = result["structuredContent"]["text"]
        .as_str()
        .expect("the answer carries the text")
        .len();
    assert!(
        shown < 2_000_000,
        "a cut that returned the file would not be a cut, got {shown} bytes"
    );

    let framed = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "result": result,
    });
    let bytes = serde_json::to_vec(&framed)
        .expect("a result serialises")
        .len();
    assert!(
        bytes <= page::RESPONSE_CEILING,
        "the response framed to {bytes} bytes, over the {} Vercel will carry",
        page::RESPONSE_CEILING
    );
}

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

#[tokio::test]
async fn an_empty_file_comes_back_empty_rather_than_refused_as_a_directory() {
    let result = call(json!({
        "registry": "npm",
        "package": "empty-file",
        "version": "1.0.0",
        "path": "empty.txt",
    }))
    .await;

    assert_eq!(
        result["isError"],
        json!(false),
        "an empty file is a file with nothing in it, got {result}"
    );
    assert_eq!(
        result["structuredContent"],
        json!({ "text": "", "truncated": false, "bytes": 0, "validUtf8": true }),
        "got {result}"
    );
}

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

#[tokio::test]
async fn the_handler_answers_with_a_typed_excerpt() {
    let content = GetFileContent::call(
        Args {
            registry: Registry::Crates,
            package: PackageName::new("serde"),
            version: VersionName::new("1.0.0"),
            path: "src/lib.rs".to_owned(),
            max_bytes: None,
        },
        &Call::new(&Ctx::fixture(FIXTURES)),
    )
    .await
    .expect("the fixture set has this crate");

    assert_eq!(
        content.excerpt,
        Excerpt {
            text: "pub fn serialize() {}\n".to_owned(),
            truncated: false,
            bytes: 22,
        },
    );
    assert!(content.valid_utf8);
}

#[tokio::test]
async fn the_handler_returns_the_failure_that_says_the_path_is_a_directory() {
    let failure = GetFileContent::call(
        Args {
            registry: Registry::Crates,
            package: PackageName::new("serde"),
            version: VersionName::new("1.0.0"),
            path: "src".to_owned(),
            max_bytes: None,
        },
        &Call::new(&Ctx::fixture(FIXTURES)),
    )
    .await
    .expect_err("`src` is a directory of this crate's");

    match failure {
        Failure::PathIsDirectory { path, .. } => assert_eq!(path, "src"),
        other => panic!("a directory should say so, got {other:?}"),
    }
}

#[tokio::test]
async fn the_handler_returns_the_failure_that_names_the_absent_path() {
    let failure = GetFileContent::call(
        Args {
            registry: Registry::Crates,
            package: PackageName::new("serde"),
            version: VersionName::new("1.0.0"),
            path: "src/nowhere.rs".to_owned(),
            max_bytes: None,
        },
        &Call::new(&Ctx::fixture(FIXTURES)),
    )
    .await
    .expect_err("this crate has no such file");

    match failure {
        Failure::NoSuchFile {
            package,
            version,
            path,
        } => {
            assert_eq!(package, "serde");
            assert_eq!(version, "1.0.0");
            assert_eq!(path, "src/nowhere.rs");
        }
        other => panic!("an absent path should say so, got {other:?}"),
    }
}

async fn listed(name: &str) -> Value {
    Client::fixture().listed(name).await
}

async fn call(arguments: Value) -> Value {
    Client::fixture().call(TOOL, arguments).await
}
