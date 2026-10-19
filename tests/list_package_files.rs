mod common;

use common::{Client, FIXTURES};
use diffpack_server::error::Failure;
use diffpack_server::page;
use diffpack_server::registry::{PackageName, Registry, VersionName};
use diffpack_server::tools::list_package_files::{Args, Entry, EntryType, ListPackageFiles};
use diffpack_server::tools::Tool;
use diffpack_server::tools::{Call, Ctx};
use serde_json::{json, Value};

const TOOL: &str = "list_package_files";

#[tokio::test]
async fn the_definition_carries_everything_an_agent_needs() {
    let tool = listed(TOOL).await;

    for field in [
        "registry", "package", "version", "prefix", "cursor", "limit",
    ] {
        assert!(
            tool["inputSchema"]["properties"][field].is_object(),
            "the input schema should describe `{field}`, got {}",
            tool["inputSchema"]
        );
    }
    for required in ["registry", "package", "version"] {
        assert!(
            tool["inputSchema"]["required"]
                .as_array()
                .is_some_and(|fields| fields.iter().any(|field| field == required)),
            "`{required}` is not optional, got {}",
            tool["inputSchema"]
        );
    }
    assert_eq!(
        tool["annotations"]["idempotentHint"], true,
        "a published version's contents are immutable, got {}",
        tool["annotations"]
    );
}

#[tokio::test]
async fn the_description_says_the_top_level_directory_is_stripped() {
    let tool = listed(TOOL).await;
    let description = tool["description"].as_str().unwrap_or_else(|| {
        panic!("a tool an agent picks without documentation has one, got {tool}")
    });

    assert!(
        description.contains("src/index.js") && description.contains("zod-4.0.0/src/index.js"),
        "the description should show the shape of a path rather than describe \
         it, since the agent's mistake is a guessed path: got {description}"
    );
}

#[tokio::test]
async fn the_paging_arguments_document_the_numbers_that_bind() {
    let tool = listed(TOOL).await;
    let limit = &tool["inputSchema"]["properties"]["limit"];

    assert_eq!(limit["default"], json!(page::DEFAULT_LIMIT), "got {limit}");
    assert_eq!(limit["maximum"], json!(page::MAX_LIMIT), "got {limit}");
    assert_eq!(limit["minimum"], json!(1), "got {limit}");

    assert!(
        limit["description"]
            .as_str()
            .is_some_and(|said| said.contains("clamped")),
        "the description is the one `page::Limit` writes, got {limit}"
    );

    let cursor = &tool["inputSchema"]["properties"]["cursor"];
    assert_eq!(cursor["pattern"], "^p1:[0-9]+$", "got {cursor}");
    assert!(
        cursor["description"]
            .as_str()
            .is_some_and(|said| said.contains("unchanged")),
        "the description is the one `page::Cursor` writes, got {cursor}"
    );
}

#[tokio::test]
async fn an_npm_package_is_listed_with_the_root_directory_stripped() {
    let result = call(json!({
        "registry": "npm",
        "package": "@types/node",
        "version": "20.1.0",
    }))
    .await;

    assert_eq!(
        result["isError"],
        json!(false),
        "a package the fixture set has is not an error, got {result}"
    );

    assert_eq!(
        paths(&result),
        vec!["index.d.ts", "package.json"],
        "npm wraps a package in `package/`, and a listing that kept it would \
         make two versions of the same package incomparable: got {result}"
    );
}

#[tokio::test]
async fn a_crate_is_listed_with_the_root_directory_stripped() {
    let result = call(json!({
        "registry": "crates",
        "package": "serde",
        "version": "1.0.0",
    }))
    .await;

    assert_eq!(
        paths(&result),
        vec!["Cargo.toml", "src", "src/lib.rs"],
        "a `.crate` wraps everything in `serde-1.0.0/`, got {result}"
    );
}

#[tokio::test]
async fn a_pypi_package_is_listed_through_the_metadata_hop() {
    let result = call(json!({
        "registry": "pypi",
        "package": "requests",
        "version": "2.31.0",
    }))
    .await;

    assert_eq!(
        paths(&result),
        vec!["requests", "requests/__init__.py", "setup.py"],
        "the sdist wraps everything in `requests-2.31.0/`, and `setup.py` is \
         the file only the sdist has: got {result}"
    );
}

#[tokio::test]
async fn an_entry_says_what_it_is_and_how_big_it_is() {
    let result = call(json!({
        "registry": "crates",
        "package": "serde",
        "version": "1.0.0",
    }))
    .await;

    assert_eq!(
        result["structuredContent"]["items"],
        json!([
            { "path": "Cargo.toml", "type": "file", "size": 43 },
            { "path": "src", "type": "directory", "size": 0 },
            { "path": "src/lib.rs", "type": "file", "size": 22 },
        ]),
        "got {result}"
    );
}

#[tokio::test]
async fn an_empty_file_is_a_file_of_no_bytes_and_not_a_directory() {
    let result = call(json!({
        "registry": "npm",
        "package": "empty-file",
        "version": "1.0.0",
    }))
    .await;

    assert_eq!(
        result["structuredContent"]["items"],
        json!([
            { "path": "empty.txt", "type": "file", "size": 0 },
            { "path": "src", "type": "directory", "size": 0 },
            { "path": "src/index.js", "type": "file", "size": 11 },
        ]),
        "got {result}"
    );
}

#[tokio::test]
async fn a_prefix_narrows_the_listing_to_that_subtree() {
    let result = call(json!({
        "registry": "crates",
        "package": "serde",
        "version": "1.0.0",
        "prefix": "src",
    }))
    .await;

    assert_eq!(
        paths(&result),
        vec!["src/lib.rs"],
        "`src` holds one file and is not itself under `src`, got {result}"
    );
    assert_eq!(
        result["structuredContent"]["total"],
        json!(1),
        "the total is the narrowed sequence's length, not the archive's — a \
         caller told 3 has no way to know it was shown all there was: got {result}"
    );
}

#[tokio::test]
async fn a_prefix_is_a_directory_rather_than_the_first_few_characters() {
    let result = call(json!({
        "registry": "crates",
        "package": "serde",
        "version": "1.0.0",
        "prefix": "sr",
    }))
    .await;

    assert_eq!(
        paths(&result),
        Vec::<&str>::new(),
        "`sr` is not a directory of this crate's, got {result}"
    );
    assert_eq!(
        result["isError"],
        json!(false),
        "a directory a package does not have is an empty listing, not a \
         failure — there is nothing for a model to fix: got {result}"
    );
}

#[tokio::test]
async fn a_trailing_slash_on_a_prefix_makes_no_difference() {
    let bare = call(json!({
        "registry": "crates", "package": "serde", "version": "1.0.0", "prefix": "src",
    }))
    .await;
    let slashed = call(json!({
        "registry": "crates", "package": "serde", "version": "1.0.0", "prefix": "src/",
    }))
    .await;

    assert_eq!(
        slashed["structuredContent"], bare["structuredContent"],
        "`src/` and `src` name the same directory"
    );
}

#[tokio::test]
async fn a_prefix_of_a_slash_is_the_whole_archive() {
    let whole = call(json!({
        "registry": "crates", "package": "serde", "version": "1.0.0",
    }))
    .await;
    let rooted = call(json!({
        "registry": "crates", "package": "serde", "version": "1.0.0", "prefix": "/",
    }))
    .await;

    assert_eq!(
        whole["structuredContent"]["total"],
        json!(3),
        "the control: the archive has three entries, got {whole}"
    );
    assert_eq!(
        rooted["structuredContent"], whole["structuredContent"],
        "`/` is the root, not a directory nothing is under"
    );
}

#[tokio::test]
async fn an_empty_prefix_is_the_whole_archive() {
    let whole = call(json!({
        "registry": "crates", "package": "serde", "version": "1.0.0",
    }))
    .await;
    let empty = call(json!({
        "registry": "crates", "package": "serde", "version": "1.0.0", "prefix": "",
    }))
    .await;

    assert_eq!(
        whole["structuredContent"]["total"],
        json!(3),
        "the control: the archive has three entries, got {whole}"
    );
    assert_eq!(
        empty["structuredContent"], whole["structuredContent"],
        "`\"\"` is the root, not a directory named nothing"
    );
}

#[tokio::test]
async fn a_null_prefix_is_the_whole_archive() {
    let whole = call(json!({
        "registry": "crates", "package": "serde", "version": "1.0.0",
    }))
    .await;
    let null = call(json!({
        "registry": "crates", "package": "serde", "version": "1.0.0", "prefix": null,
    }))
    .await;

    assert_eq!(
        whole["structuredContent"]["total"],
        json!(3),
        "the control: the archive has three entries, got {whole}"
    );
    assert_eq!(
        null["structuredContent"], whole["structuredContent"],
        "`null` is the argument left out"
    );
}

#[tokio::test]
async fn a_walk_of_the_pages_is_the_whole_listing() {
    let whole = paths(
        &call(json!({
            "registry": "crates", "package": "serde", "version": "1.0.0",
        }))
        .await,
    )
    .iter()
    .map(|path| path.to_string())
    .collect::<Vec<_>>();

    let first = call(json!({
        "registry": "crates", "package": "serde", "version": "1.0.0", "limit": 2,
    }))
    .await;

    assert_eq!(
        paths(&first),
        &whole[..2],
        "a page of two is the first two, got {first}"
    );
    assert_eq!(
        first["structuredContent"]["total"],
        json!(3),
        "the total is the sequence's length and not the page's — an agent \
         told it received 2 of 2 has no reason to ask again: got {first}"
    );

    let cursor = first["structuredContent"]["nextCursor"]
        .as_str()
        .unwrap_or_else(|| panic!("a page that ends early hands back a cursor, got {first}"))
        .to_owned();

    let rest = call(json!({
        "registry": "crates", "package": "serde", "version": "1.0.0", "cursor": cursor,
    }))
    .await;

    assert_eq!(
        paths(&rest),
        &whole[2..],
        "the cursor resumes where the page stopped, got {rest}"
    );
    assert!(
        rest["structuredContent"]["nextCursor"].is_null(),
        "the last page of a sequence does not hand out another cursor, got {rest}"
    );
}

#[tokio::test]
async fn a_cursor_a_client_invented_never_reaches_the_handler() {
    let answer = Client::fixture()
        .post(json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "tools/call",
            "params": {
                "name": TOOL,
                "arguments": {
                    "registry": "crates", "package": "serde", "version": "1.0.0", "cursor": "2",
                },
            },
        }))
        .await;

    assert_eq!(
        answer["error"]["code"], -32602,
        "a cursor that is not ours is the client's mistake, got {answer}"
    );
}

#[tokio::test]
async fn a_cursor_in_the_shape_the_server_writes_is_followed_wherever_it_came_from() {
    let serde = json!({ "registry": "crates", "package": "serde", "version": "1.0.0" });
    let whole = paths(&call(serde.clone()).await)
        .iter()
        .map(|path| path.to_string())
        .collect::<Vec<_>>();

    let mut from_the_second = serde;
    from_the_second["cursor"] = json!("p1:1");
    let resumed = call(from_the_second).await;

    assert_eq!(
        paths(&resumed),
        &whole[1..],
        "a cursor is only checked for its shape, so one written by hand \
         resumes where it says: got {resumed}"
    );

    let cursor = &listed(TOOL).await["inputSchema"]["properties"]["cursor"];
    let description = cursor["description"].as_str().unwrap_or_default();
    assert!(
        !description.contains("by hand is refused"),
        "the description must not promise a refusal the server does not \
         make, got {cursor}"
    );
}

#[tokio::test]
async fn a_package_with_thousands_of_files_answers_with_a_page_that_fits() {
    let result = call(json!({
        "registry": "npm",
        "package": "many-files",
        "version": "1.0.0",
        "limit": page::MAX_LIMIT,
    }))
    .await;

    let total = result["structuredContent"]["total"]
        .as_u64()
        .unwrap_or_else(|| panic!("a page states the whole sequence's length, got {result}"));
    assert_eq!(
        total, 2527,
        "2500 files, `src`, and the 26 directories under it — the archive's \
         own count, read with `tar` rather than from this server"
    );

    let returned = paths(&result).len();
    assert_eq!(
        returned,
        page::MAX_LIMIT,
        "a page holds what was asked for and no more, got {returned}"
    );
    assert!(
        result["structuredContent"]["nextCursor"].is_string(),
        "a listing this tool could not finish has to say where to resume, got \
         a page of {returned} out of {total}"
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
async fn the_order_is_the_same_on_every_call() {
    let arguments = json!({
        "registry": "npm", "package": "many-files", "version": "1.0.0", "limit": 500,
    });

    let first = call(arguments.clone()).await;
    let again = call(arguments).await;

    assert_eq!(
        first["structuredContent"], again["structuredContent"],
        "two calls with the same arguments are the same page"
    );

    let mut sorted = paths(&first);
    sorted.sort_unstable();
    assert_eq!(
        paths(&first),
        sorted,
        "the order is the paths' own, which is the only one two calls can agree on"
    );
}

#[tokio::test]
async fn a_version_the_registry_does_not_have_is_a_tool_error_naming_it() {
    let result = call(json!({
        "registry": "npm",
        "package": "zod",
        "version": "99.99.99",
    }))
    .await;

    assert_eq!(
        result["isError"],
        json!(true),
        "the model is the one who can pick another version, got {result}"
    );

    let text = result["content"][0]["text"]
        .as_str()
        .expect("a tool error carries text for the model");
    for named in ["zod", "99.99.99"] {
        assert!(
            text.contains(named),
            "the message should name `{named}`, which is what was not found: got {text}"
        );
    }
}

#[tokio::test]
async fn a_package_the_registry_does_not_have_is_a_tool_error_naming_it() {
    let result = call(json!({
        "registry": "crates",
        "package": "not-a-real-crate",
        "version": "1.0.0",
    }))
    .await;

    assert_eq!(result["isError"], json!(true), "got {result}");

    let text = result["content"][0]["text"]
        .as_str()
        .expect("a tool error carries text for the model");
    assert!(
        text.contains("not-a-real-crate"),
        "the message should name what was asked for, got {text}"
    );
}

#[tokio::test]
async fn the_handler_answers_with_typed_entries() {
    let page = ListPackageFiles::call(
        Args {
            registry: Registry::Crates,
            package: PackageName::new("serde"),
            version: VersionName::new("1.0.0"),
            prefix: serde_json::from_value(json!("src")).expect("a directory"),
            cursor: None,
            limit: None,
        },
        &Call::new(&Ctx::fixture(FIXTURES)),
    )
    .await
    .expect("the fixture set has this crate");

    assert_eq!(
        page.items,
        vec![Entry {
            path: "src/lib.rs".to_owned(),
            entry_type: EntryType::File,
            size: 22,
        }],
    );
    assert_eq!(page.total, 1);
    assert_eq!(page.next_cursor, None);
}

#[tokio::test]
async fn the_handler_returns_the_failure_that_names_what_was_not_found() {
    let failure = ListPackageFiles::call(
        Args {
            registry: Registry::Npm,
            package: PackageName::new("zod"),
            version: VersionName::new("99.99.99"),
            prefix: None,
            cursor: None,
            limit: None,
        },
        &Call::new(&Ctx::fixture(FIXTURES)),
    )
    .await
    .expect_err("the fixture set says this URL serves nothing");

    match failure {
        Failure::NoSuchVersion {
            package, version, ..
        } => {
            assert_eq!(package, "zod");
            assert_eq!(version, "99.99.99");
        }
        other => panic!("a missing version should say so, got {other:?}"),
    }
}

fn paths(result: &Value) -> Vec<&str> {
    result["structuredContent"]["items"]
        .as_array()
        .unwrap_or_else(|| panic!("a page carries its items, got {result}"))
        .iter()
        .map(|entry| {
            entry["path"]
                .as_str()
                .unwrap_or_else(|| panic!("every entry names a path, got {entry}"))
        })
        .collect()
}

async fn listed(name: &str) -> Value {
    Client::fixture().listed(name).await
}

async fn call(arguments: Value) -> Value {
    Client::fixture().call(TOOL, arguments).await
}
