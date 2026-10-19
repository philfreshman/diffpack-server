mod common;

use common::Client;
use diffpack_server::engine;
use diffpack_server::handle::{DiffHandle, Inputs};
use diffpack_server::registry::Registry;
use serde_json::{json, Value};

const TOOL: &str = "diff_package_versions";

#[tokio::test]
async fn the_definition_carries_everything_an_agent_needs() {
    let tool = listed(TOOL).await;

    for field in [
        "registry",
        "package",
        "from_version",
        "to_version",
        "similarity_threshold",
        "ignore_whitespace",
    ] {
        assert!(
            tool["inputSchema"]["properties"][field].is_object(),
            "the input schema should describe `{field}`, got {}",
            tool["inputSchema"]
        );
    }

    for required in ["registry", "package", "from_version", "to_version"] {
        assert!(
            tool["inputSchema"]["required"]
                .as_array()
                .is_some_and(|fields| fields.iter().any(|field| field == required)),
            "`{required}` is not optional, got {}",
            tool["inputSchema"]
        );
    }

    assert_eq!(
        tool["inputSchema"]["properties"]["similarity_threshold"]["default"],
        json!(0.75),
        "the default is what `diffpack`'s own worker passes, got {}",
        tool["inputSchema"]
    );
    assert_eq!(
        tool["inputSchema"]["properties"]["ignore_whitespace"]["default"],
        json!(false),
        "got {}",
        tool["inputSchema"]
    );

    let threshold = &tool["inputSchema"]["properties"]["similarity_threshold"];
    assert_eq!(threshold["minimum"], json!(0.0), "got {threshold}");
    assert_eq!(threshold["maximum"], json!(1.0), "got {threshold}");
    assert_eq!(
        tool["annotations"]["idempotentHint"], true,
        "two published versions are immutable, so the same arguments give the \
         same diff, got {}",
        tool["annotations"]
    );
}

#[tokio::test]
async fn the_description_says_which_version_is_which_and_that_order_matters() {
    let tool = listed(TOOL).await;
    let said = tool["description"].as_str().unwrap_or_default();

    for field in ["`from_version`", "`to_version`"] {
        assert!(
            said.contains(field),
            "the description should name {field}, got {said:?}"
        );
    }
    assert!(
        said.contains("swapping the two is a different comparison"),
        "the description should say a swapped pair is another comparison, \
         got {said:?}"
    );
}

#[tokio::test]
async fn the_handle_in_the_answer_is_described_by_the_module_that_mints_it() {
    let tool = listed(TOOL).await;
    let handle = &tool["outputSchema"]["properties"]["handle"];

    assert_eq!(
        handle["type"], "string",
        "a handle is one string on the wire, got {handle}"
    );
    assert_eq!(handle["pattern"], "^d1:[A-Za-z0-9_-]+$", "got {handle}");

    let said = handle["description"]
        .as_str()
        .unwrap_or_else(|| panic!("a described field, got {handle}"));
    assert!(
        said.contains("minted by"),
        "the description should say where a handle comes from: {said}"
    );
    assert!(
        said.contains("not written by hand"),
        "and that it is not one a client builds: {said}"
    );
}

#[tokio::test]
async fn a_version_pair_is_summarised_by_status_and_by_line() {
    let result = call(json!({
        "registry": "npm",
        "package": "diffable",
        "from_version": "1.0.0",
        "to_version": "2.0.0",
    }))
    .await;

    assert_eq!(
        result["isError"],
        json!(false),
        "a pair the fixture set has is not an error, got {result}"
    );

    let totals = &result["structuredContent"]["totals"];

    assert_eq!(
        totals,
        &json!({
            "added": 1,
            "removed": 1,
            "modified": 1,
            "renamed": 1,
            "unchanged": 1,
            "lines_added": 2,
            "lines_removed": 2,
        }),
        "the fixture has one file of each status, and four lines move in \
         total: got {result}"
    );
}

#[tokio::test]
async fn a_go_module_pair_reports_what_changed_rather_than_everything_moved() {
    let result = call(json!({
        "registry": "go",
        "package": "github.com/Masterminds/semver/v3",
        "from_version": "v3.2.0",
        "to_version": "v3.2.1",
    }))
    .await;

    assert_eq!(
        result["isError"],
        json!(false),
        "a module path with capitals is fetched from the proxy's escaped path, \
         got {result}"
    );

    assert_eq!(
        result["structuredContent"]["totals"],
        json!({
            "added": 1,
            "removed": 0,
            "modified": 1,
            "renamed": 0,
            "unchanged": 3,
            "lines_added": 4,
            "lines_removed": 1,
        }),
        "every entry of a module zip sits under `{{module}}@{{version}}/`, a \
         prefix that differs between the two versions, so unless it is \
         stripped no path is shared and all four files of v3.2.0 read as \
         removed: got {result}"
    );
}

#[tokio::test]
async fn the_answer_names_the_pair_it_compared() {
    let result = call(json!({
        "registry": "npm",
        "package": "diffable",
        "from_version": "1.0.0",
        "to_version": "2.0.0",
    }))
    .await;

    let answer = &result["structuredContent"];
    assert_eq!(answer["from_version"], json!("1.0.0"), "got {result}");
    assert_eq!(answer["to_version"], json!("2.0.0"), "got {result}");
}

#[tokio::test]
async fn the_most_changed_files_are_ranked_and_a_rename_says_where_it_came_from() {
    let result = call(json!({
        "registry": "npm",
        "package": "diffable",
        "from_version": "1.0.0",
        "to_version": "2.0.0",
    }))
    .await;

    assert_eq!(
        result["structuredContent"]["most_changed"],
        json!([
            {
                "path": "src/index.js",
                "status": "modified",
                "lines_added": 1,
                "lines_removed": 1,
            },
            {
                "path": "src/added.js",
                "status": "added",
                "lines_added": 1,
                "lines_removed": 0,
            },
            {
                "path": "src/removed.js",
                "status": "removed",
                "lines_added": 0,
                "lines_removed": 1,
            },
            {
                "path": "src/new-name.js",
                "status": "renamed",
                "lines_added": 0,
                "lines_removed": 0,
                "old_path": "src/old-name.js",
            },
        ]),
        "ranked by churn and then by path, with `README.md` absent because it \
         did not change: got {result}"
    );
}

#[tokio::test]
async fn a_rename_is_reported_as_one_moved_file() {
    let result = call(json!({
        "registry": "npm",
        "package": "diffable",
        "from_version": "1.0.0",
        "to_version": "2.0.0",
    }))
    .await;

    let answer = &result["structuredContent"];

    assert_eq!(answer["totals"]["renamed"], json!(1), "got {result}");

    let renamed: Vec<&Value> = answer["most_changed"]
        .as_array()
        .unwrap_or_else(|| panic!("the answer lists what changed, got {result}"))
        .iter()
        .filter(|file| file["status"] == "renamed")
        .collect();

    assert_eq!(renamed.len(), 1, "got {result}");
    assert_eq!(
        renamed[0]["old_path"],
        json!("src/old-name.js"),
        "the old path is the half an agent cannot work out from the new one, \
         and it is what makes the move readable: got {result}"
    );
}

#[tokio::test]
async fn the_summary_lists_a_bounded_sample_however_much_changed() {
    let result = call(json!({
        "registry": "npm",
        "package": "churny",
        "from_version": "1.0.0",
        "to_version": "2.0.0",
    }))
    .await;

    assert_eq!(
        result["structuredContent"]["totals"]["modified"],
        json!(25),
        "the totals count every file, cut or not — that is what they are for: \
         got {result}"
    );

    let listed = result["structuredContent"]["most_changed"]
        .as_array()
        .unwrap_or_else(|| panic!("the answer lists what changed, got {result}"))
        .clone();

    assert_eq!(
        listed.len(),
        20,
        "the listing is a bounded sample, got {result}"
    );

    let paths: Vec<&str> = listed
        .iter()
        .filter_map(|file| file["path"].as_str())
        .collect();
    let expected: Vec<String> = (0..20).map(|i| format!("src/file{i:02}.js")).collect();
    assert_eq!(
        paths, expected,
        "equal churn is broken by path, so the same pair gives the same \
         twenty every time: got {result}"
    );
}

#[tokio::test]
async fn the_answer_says_whether_it_was_served_from_cache() {
    let result = call(json!({
        "registry": "npm",
        "package": "diffable",
        "from_version": "1.0.0",
        "to_version": "2.0.0",
    }))
    .await;

    assert_eq!(
        result["structuredContent"]["cached"],
        json!(false),
        "a pair this process has not diffed before was computed now, got \
         {result}"
    );
}

#[tokio::test]
async fn a_version_that_does_not_exist_is_an_error_the_model_sees() {
    let result = call(json!({
        "registry": "npm",
        "package": "diffable",
        "from_version": "1.0.0",
        "to_version": "9.9.9",
    }))
    .await;

    assert_eq!(
        result["isError"],
        json!(true),
        "the model picked the version, so the model is told: got {result}"
    );

    let said = result["content"][0]["text"]
        .as_str()
        .unwrap_or_else(|| panic!("a tool error carries text a model reads, got {result}"));

    assert!(
        said.contains("9.9.9") && said.contains("diffable"),
        "the refusal names what was not found, so the model can correct it \
         without guessing which of the two versions was wrong: got {said}"
    );
}

#[tokio::test]
async fn a_missing_version_tells_the_model_where_to_find_one_that_exists() {
    let result = call(json!({
        "registry": "npm",
        "package": "diffable",
        "from_version": "1.0.0",
        "to_version": "9.9.9",
    }))
    .await;

    assert_eq!(result["isError"], json!(true), "got {result}");

    let said = result["content"][0]["text"]
        .as_str()
        .unwrap_or_else(|| panic!("a tool error carries text a model reads, got {result}"));

    assert!(
        said.contains("`list_package_versions`"),
        "a refusal the model can only guess its way past is not one it recovers \
         from, so it names the tool that lists the versions there are: got {said}"
    );
}

#[tokio::test]
async fn the_refusal_names_whichever_of_the_two_versions_is_missing() {
    let result = call(json!({
        "registry": "npm",
        "package": "diffable",
        "from_version": "9.9.9",
        "to_version": "2.0.0",
    }))
    .await;

    assert_eq!(result["isError"], json!(true), "got {result}");

    let said = result["content"][0]["text"]
        .as_str()
        .unwrap_or_else(|| panic!("a tool error carries text a model reads, got {result}"));

    assert!(said.contains("9.9.9"), "got {said}");
    assert!(
        !said.contains("2.0.0"),
        "the version that does exist is not what is missing, and naming it \
         sends the model to correct the wrong argument: got {said}"
    );
}

#[tokio::test]
async fn ignoring_whitespace_drops_a_reformatting_out_of_the_answer() {
    let counted = call(json!({
        "registry": "npm",
        "package": "reformatted",
        "from_version": "1.0.0",
        "to_version": "2.0.0",
        "ignore_whitespace": false,
    }))
    .await;

    assert_eq!(
        counted["structuredContent"]["totals"]["modified"],
        json!(1),
        "the line does differ, byte for byte: got {counted}"
    );
    assert_eq!(
        counted["structuredContent"]["totals"]["lines_added"],
        json!(1),
        "got {counted}"
    );

    let ignored = call(json!({
        "registry": "npm",
        "package": "reformatted",
        "from_version": "1.0.0",
        "to_version": "2.0.0",
        "ignore_whitespace": true,
    }))
    .await;

    let totals = &ignored["structuredContent"]["totals"];
    assert_eq!(
        totals["modified"],
        json!(0),
        "every difference is whitespace, so there is nothing left to report: \
         got {ignored}"
    );
    assert_eq!(
        totals["unchanged"],
        json!(1),
        "the file is still there — it is unchanged, not absent: got {ignored}"
    );
    assert_eq!(
        ignored["structuredContent"]["most_changed"],
        json!([]),
        "got {ignored}"
    );

    assert_ne!(
        counted["structuredContent"]["diff_id"], ignored["structuredContent"]["diff_id"],
        "the argument changes the statuses, so it changes what names the \
         diff: got {counted} and {ignored}"
    );
}

#[tokio::test]
async fn lowering_the_similarity_threshold_finds_more_renames() {
    let strict = call(json!({
        "registry": "npm",
        "package": "renamey",
        "from_version": "1.0.0",
        "to_version": "2.0.0",
    }))
    .await;

    let totals = &strict["structuredContent"]["totals"];
    assert_eq!(
        totals["renamed"],
        json!(0),
        "at the default the pair is not alike enough: got {strict}"
    );
    assert_eq!(totals["added"], json!(1), "got {strict}");
    assert_eq!(totals["removed"], json!(1), "got {strict}");

    let lenient = call(json!({
        "registry": "npm",
        "package": "renamey",
        "from_version": "1.0.0",
        "to_version": "2.0.0",
        "similarity_threshold": 0.4,
    }))
    .await;

    let totals = &lenient["structuredContent"]["totals"];
    assert_eq!(
        totals["renamed"],
        json!(1),
        "at 0.4 the same pair is one file that moved: got {lenient}"
    );
    assert_eq!(
        (&totals["added"], &totals["removed"]),
        (&json!(0), &json!(0)),
        "and it is no longer counted as a delete beside an add, which is the \
         half that would otherwise double-count it: got {lenient}"
    );

    let moved = &lenient["structuredContent"]["most_changed"][0];
    assert_eq!(moved["path"], json!("src/processor.js"), "got {lenient}");
    assert_eq!(moved["old_path"], json!("src/handler.js"), "got {lenient}");

    assert_ne!(
        strict["structuredContent"]["diff_id"], lenient["structuredContent"]["diff_id"],
        "the threshold changes the statuses, so it changes what names the \
         diff — which is what the answer tells an agent it does: got {strict} \
         and {lenient}"
    );
}

#[tokio::test]
async fn the_diff_id_is_the_one_the_cache_key_document_fixes() {
    let vector = worked_example_at_the_pinned_engine();
    let input = &vector["input"];

    let result = call(json!({
        "registry": input["registry"],
        "package": input["package"],
        "from_version": input["from"],
        "to_version": input["to"],
        "similarity_threshold": input["similarity_threshold"],
        "ignore_whitespace": input["ignore_whitespace"],
    }))
    .await;

    assert_eq!(
        result["structuredContent"]["diff_id"], vector["diff_id"],
        "the identifier is `docs/cache-key.md`'s, so that a result written by \
         this server can be found by an implementation that shares no code \
         with it: got {result}"
    );
}

#[tokio::test]
async fn the_same_inputs_always_name_the_same_diff() {
    let arguments = json!({
        "registry": "npm",
        "package": "diffable",
        "from_version": "1.0.0",
        "to_version": "2.0.0",
    });

    let first = call(arguments.clone()).await;
    let again = call(arguments).await;

    assert_eq!(
        first["structuredContent"]["diff_id"], again["structuredContent"]["diff_id"],
        "got {first} and {again}"
    );
    assert_eq!(
        first["structuredContent"]["handle"], again["structuredContent"]["handle"],
        "the handle is minted from the inputs, so it is stable too: got \
         {first} and {again}"
    );
}

#[tokio::test]
async fn swapping_the_versions_is_a_different_diff() {
    let forwards = call(json!({
        "registry": "npm",
        "package": "diffable",
        "from_version": "1.0.0",
        "to_version": "2.0.0",
    }))
    .await;

    let backwards = call(json!({
        "registry": "npm",
        "package": "diffable",
        "from_version": "2.0.0",
        "to_version": "1.0.0",
    }))
    .await;

    assert_ne!(
        forwards["structuredContent"]["diff_id"], backwards["structuredContent"]["diff_id"],
        "the order is part of what names a diff, got {forwards} and {backwards}"
    );

    let there = &forwards["structuredContent"]["totals"];
    let back = &backwards["structuredContent"]["totals"];

    assert_eq!(
        there["added"], back["removed"],
        "got {forwards}, {backwards}"
    );
    assert_eq!(
        there["removed"], back["added"],
        "got {forwards}, {backwards}"
    );
    assert_eq!(
        there["lines_added"], back["lines_removed"],
        "got {forwards}, {backwards}"
    );
    assert_eq!(
        there["lines_removed"], back["lines_added"],
        "got {forwards}, {backwards}"
    );
}

#[tokio::test]
async fn the_handle_carries_the_inputs_it_was_minted_from() {
    let result = call(json!({
        "registry": "npm",
        "package": "diffable",
        "from_version": "1.0.0",
        "to_version": "2.0.0",
        "similarity_threshold": 0.4,
        "ignore_whitespace": true,
    }))
    .await;

    let encoded = result["structuredContent"]["handle"]
        .as_str()
        .unwrap_or_else(|| panic!("the answer carries a handle, got {result}"));

    let handle = DiffHandle::decode(encoded)
        .unwrap_or_else(|e| panic!("this server minted it, so it reads it back: {e:?}"));

    assert_eq!(
        handle.inputs(),
        &Inputs {
            registry: Registry::Npm,
            package: "diffable".to_owned(),
            from_version: "1.0.0".to_owned(),
            to_version: "2.0.0".to_owned(),
            similarity_threshold: 0.4,
            ignore_whitespace: true,
        },
        "got {result}"
    );

    assert_eq!(
        result["structuredContent"]["diff_id"],
        json!(handle.diff_id()),
        "the bare identifier beside the handle is the handle's own, got {result}"
    );
}

fn golden_vector(name: &str) -> Value {
    golden_vectors()
        .into_iter()
        .find(|vector| vector["name"] == name)
        .unwrap_or_else(|| panic!("`{name}` should be one of the golden vectors"))
}

fn worked_example_at_the_pinned_engine() -> Value {
    let mut pinned = golden_vector("npm/zod")["input"].clone();
    pinned["engine"] = json!(engine::VERSION);

    golden_vectors()
        .into_iter()
        .find(|vector| vector["input"] == pinned)
        .unwrap_or_else(|| {
            panic!(
                "no vector holds the worked example at engine {}: add one to \
                 fixtures/cache-key-vectors.json, generated from docs/cache-key.md \
                 rather than from this crate",
                engine::VERSION
            )
        })
}

fn golden_vectors() -> Vec<Value> {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/fixtures/cache-key-vectors.json"
    );
    let text = std::fs::read_to_string(path).expect("the golden vectors should read");
    let mut file: Value = serde_json::from_str(&text).expect("the golden vectors should parse");

    match file["vectors"].take() {
        Value::Array(vectors) => vectors,
        other => panic!("the file holds an array of vectors, not {other}"),
    }
}

async fn listed(name: &str) -> Value {
    Client::fixture().listed(name).await
}

async fn call(arguments: Value) -> Value {
    Client::fixture().call(TOOL, arguments).await
}
