mod common;

use common::{names, Client, FIXTURES};
use diffpack_server::error::Failure;
use diffpack_server::page;
use diffpack_server::registry::Registry;
use diffpack_server::tools::search_packages::{Args, SearchPackages};
use diffpack_server::tools::{Call, Ctx, Tool};
use serde_json::{json, Value};

const TOOL: &str = "search_packages";

#[tokio::test]
async fn the_handler_returns_the_failure_that_says_the_source_is_unwell() {
    let failure = SearchPackages::call(
        Args {
            registry: Registry::Npm,
            query: "outage".to_owned(),
            cursor: None,
            limit: None,
        },
        &Call::new(&Ctx::fixture(FIXTURES)),
    )
    .await
    .expect_err("this source is not answering");

    match failure {
        Failure::Unavailable { registry, .. } => assert_eq!(registry, "npm"),
        other => panic!("a source that did not answer should say so, got {other:?}"),
    }
}

#[tokio::test]
async fn the_definition_carries_everything_an_agent_needs() {
    let tool = listed(TOOL).await;

    for field in ["registry", "query", "limit"] {
        assert!(
            tool["inputSchema"]["properties"][field].is_object(),
            "the input schema should describe `{field}`, got {}",
            tool["inputSchema"]
        );
    }
    for required in ["registry", "query"] {
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
            .is_some_and(|fields| !fields.iter().any(|field| field == "limit")),
        "a limit that has a default is not something a caller has to supply, got {}",
        tool["inputSchema"]
    );

    for field in ["items", "total"] {
        assert!(
            tool["outputSchema"]["properties"][field].is_object(),
            "the output schema should describe `{field}`, got {}",
            tool["outputSchema"]
        );
    }
    for field in ["name", "version", "description"] {
        assert!(
            tool["outputSchema"]["properties"]["items"]["items"]["properties"][field].is_object(),
            "a hit should describe `{field}`, got {}",
            tool["outputSchema"]
        );
    }
}

#[tokio::test]
async fn a_search_does_not_claim_the_same_answer_twice() {
    let tool = listed(TOOL).await;

    assert_eq!(
        tool["annotations"]["idempotentHint"], false,
        "a registry's index moves under a search, got {}",
        tool["annotations"]
    );
}

#[tokio::test]
async fn the_description_says_which_registry_answers_with_less() {
    let tool = listed(TOOL).await;
    let said = tool["description"].as_str().unwrap_or_default();

    assert!(
        said.contains("PyPI"),
        "the description should name the registry that answers with less, got {said:?}"
    );
    assert!(
        said.contains("nothing else") || said.contains("a name alone"),
        "and should say what it leaves out, got {said:?}"
    );
}

#[tokio::test]
async fn the_description_says_a_go_query_has_to_be_a_whole_module_path() {
    let tool = listed(TOOL).await;
    let said = tool["description"].as_str().unwrap_or_default();

    assert!(
        names(said, "Go") && said.contains("module path"),
        "Go has nothing to search, so an agent has to be told that a Go query \
         is the module path itself, got {said:?}"
    );
}

#[tokio::test]
async fn the_description_says_no_registry_answers_with_as_many_as_limit_allows() {
    let tool = listed(TOOL).await;
    let said = tool["description"].as_str().unwrap_or_default();

    assert!(
        said.contains("250") && said.contains("100"),
        "the description should say what each registry's ceiling is, got {said:?}"
    );
    assert!(
        said.contains("total"),
        "and what that makes the total, got {said:?}"
    );
}

#[tokio::test]
async fn an_npm_search_answers_with_the_packages_it_found() {
    let result = call(json!({
        "registry": "npm",
        "query": "zod",
    }))
    .await;

    let items = &result["structuredContent"]["items"];
    assert_eq!(
        items[0],
        json!({
            "name": "zod",
            "version": "4.0.0",
            "description": "TypeScript-first schema declaration and validation library \
                            with static type inference",
        }),
        "the first hit is the package itself, with everything npm says about it, got {items}"
    );
    assert_eq!(
        items[1]["name"], "zod-to-json-schema",
        "and the rest follow in the order npm ranked them, got {items}"
    );
    assert_eq!(
        result["isError"],
        json!(false),
        "packages found is not an error, got {result}"
    );
}

#[tokio::test]
async fn a_crates_io_search_answers_with_the_crates_it_found() {
    let result = call(json!({
        "registry": "crates",
        "query": "serde",
    }))
    .await;

    let items = &result["structuredContent"]["items"];
    assert_eq!(
        items[0],
        json!({
            "name": "serde",
            "version": "1.0.229",
            "description": "A generic serialization/deserialization framework",
        }),
        "got {items}"
    );
}

#[tokio::test]
async fn a_pypi_search_answers_with_names_and_says_no_more_than_that() {
    let result = call(json!({
        "registry": "pypi",
        "query": "yaml",
    }))
    .await;

    let items = &result["structuredContent"]["items"];
    assert_eq!(
        items[0],
        json!({ "name": "yaml" }),
        "a PyPI hit is a name, and no empty version or description beside it, got {items}"
    );
    assert_eq!(
        items[1]["name"], "yamldown",
        "and the rest are ranked against the query here — `yamldown` before \
         `yamllint` because the two are the same length, got {items}"
    );
}

#[tokio::test]
async fn a_go_search_finds_the_module_a_whole_path_names() {
    let result = call(json!({
        "registry": "go",
        "query": "github.com/go-chi/chi/v5",
    }))
    .await;

    assert_eq!(
        result["structuredContent"]["items"],
        json!([{ "name": "github.com/go-chi/chi/v5", "version": "v5.5.0" }]),
        "the proxy has no search, so a Go query is a module path and the hit is \
         that module with the version `@latest` resolves: got {result}"
    );
}

#[tokio::test]
async fn a_go_query_written_as_a_url_finds_the_module_by_its_path() {
    let result = call(json!({
        "registry": "go",
        "query": "https://github.com/go-chi/chi/v5/",
    }))
    .await;

    assert_eq!(
        result["structuredContent"]["items"],
        json!([{ "name": "github.com/go-chi/chi/v5", "version": "v5.5.0" }]),
        "a scheme and a trailing slash are how a module path is pasted, not part \
         of it, and the hit's name is what every other tool is then asked for: \
         got {result}"
    );
}

#[tokio::test]
async fn a_go_query_that_names_no_module_is_no_hits_rather_than_an_outage() {
    let result = call(json!({
        "registry": "go",
        "query": "http router",
    }))
    .await;

    assert_eq!(
        result["isError"],
        json!(false),
        "the proxy answers a path with no module at it with a 404, and here \
         that is the query's miss rather than the source having moved: got {result}"
    );
    assert_eq!(
        result["structuredContent"]["items"],
        json!([]),
        "got {result}"
    );
}

#[tokio::test]
async fn a_limit_is_what_the_source_is_asked_for() {
    let result = call(json!({
        "registry": "npm",
        "query": "zod",
        "limit": 1,
    }))
    .await;

    let page = &result["structuredContent"];
    assert_eq!(
        page["items"].as_array().map(Vec::len),
        Some(1),
        "one hit was asked for, got {page}"
    );
    assert_eq!(
        page["items"][0]["name"], "zod",
        "and it is the best one, got {page}"
    );
    assert_eq!(
        page["total"], 1,
        "a total is the length of what there was to page, which is what a \
         source asked for one hit returned, got {page}"
    );
}

#[tokio::test]
async fn a_query_that_matches_nothing_is_an_empty_page() {
    let result = call(json!({
        "registry": "npm",
        "query": "zzqqxxnotapackage",
    }))
    .await;

    assert_eq!(
        result["structuredContent"]["items"],
        json!([]),
        "got {result}"
    );
    assert_eq!(result["structuredContent"]["total"], 0, "got {result}");
    assert_eq!(
        result["isError"],
        json!(false),
        "finding nothing is an answer, got {result}"
    );
}

#[tokio::test]
async fn the_paging_arguments_document_the_numbers_that_bind() {
    let tool = listed(TOOL).await;
    let limit = &tool["inputSchema"]["properties"]["limit"];

    assert_eq!(limit["default"], json!(page::DEFAULT_LIMIT), "got {limit}");
    assert_eq!(limit["maximum"], json!(page::MAX_LIMIT), "got {limit}");

    let cursor = &tool["inputSchema"]["properties"]["cursor"];
    assert_eq!(cursor["pattern"], "^p1:[0-9]+$", "got {cursor}");
}

#[tokio::test]
async fn a_cursor_resumes_the_answer_rather_than_being_refused() {
    let whole = call(json!({ "registry": "npm", "query": "zod" })).await;
    assert_eq!(
        whole["structuredContent"]["total"], 2,
        "the fixture answers this query with two, got {whole}"
    );

    let resumed = call(json!({
        "registry": "npm",
        "query": "zod",
        "cursor": page::Cursor::at(1).encode(),
    }))
    .await;

    let page = &resumed["structuredContent"];
    assert_eq!(
        page["items"].as_array().map(Vec::len),
        Some(1),
        "resuming past the first hit leaves one, got {page}"
    );
    assert_eq!(
        page["items"][0]["name"], "zod-to-json-schema",
        "and it is the one after it, got {page}"
    );
    assert_eq!(
        page["total"], 2,
        "a total is the sequence's length and not the page's, got {page}"
    );
}

#[tokio::test]
async fn a_blank_query_finds_nothing_rather_than_everything() {
    for query in ["", "   "] {
        let result = call(json!({ "registry": "pypi", "query": query })).await;

        assert_eq!(
            result["structuredContent"]["items"],
            json!([]),
            "a blank query is not a request for the whole index, got {result}"
        );
        assert_eq!(result["structuredContent"]["total"], 0, "got {result}");
        assert_eq!(
            result["isError"],
            json!(false),
            "asking for nothing is not an error, got {result}"
        );
    }
}

#[tokio::test]
async fn a_query_is_trimmed_before_it_reaches_a_registry() {
    let result = call(json!({ "registry": "npm", "query": "  zod  " })).await;

    assert_eq!(
        result["structuredContent"]["items"][0]["name"], "zod",
        "the fixture set is keyed by the untrimmed URL's absence, got {result}"
    );
}

#[tokio::test]
async fn a_source_that_is_not_answering_is_a_tool_error_a_model_can_retry() {
    let result = call(json!({
        "registry": "npm",
        "query": "outage",
    }))
    .await;

    assert_eq!(
        result["isError"],
        json!(true),
        "a source that did not answer is a failure, got {result}"
    );

    let said = result["content"][0]["text"].as_str().unwrap_or_default();
    assert!(
        said.contains("npm"),
        "the message should name the registry that went quiet, got {said:?}"
    );
    assert!(
        said.contains("Try again"),
        "and should say that trying again is worth it, got {said:?}"
    );
}

#[tokio::test]
async fn one_source_being_down_leaves_the_others_answering() {
    let down = call(json!({ "registry": "npm", "query": "outage" })).await;
    assert_eq!(down["isError"], json!(true), "got {down}");

    let up = call(json!({ "registry": "crates", "query": "serde" })).await;
    assert_eq!(
        up["structuredContent"]["items"][0]["name"], "serde",
        "the next call to another registry answers as it always did, got {up}"
    );
}

async fn listed(name: &str) -> Value {
    Client::fixture().listed(name).await
}

async fn call(arguments: Value) -> Value {
    Client::fixture().call(TOOL, arguments).await
}
