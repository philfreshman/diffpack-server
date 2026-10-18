use std::collections::HashMap;

mod common;

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine as _;
use common::Client;
use diffpack_server::handle::{DiffHandle, Inputs};
use diffpack_server::page;
use diffpack_server::registry::Registry;
use serde_json::{json, Value};

const TOOL: &str = "get_diff_tree";

const SUMMARY: &str = "diff_package_versions";

#[tokio::test]
async fn the_definition_carries_everything_an_agent_needs() {
    let tool = listed(TOOL).await;

    for field in ["handle", "path", "depth", "status", "cursor", "limit"] {
        assert!(
            tool["inputSchema"]["properties"][field].is_object(),
            "the input schema should describe `{field}`, got {}",
            tool["inputSchema"]
        );
    }

    assert_eq!(
        tool["inputSchema"]["required"],
        json!(["handle"]),
        "the handle is the only thing a caller must pass — every other \
         argument narrows an answer that is complete without it: got {}",
        tool["inputSchema"]
    );
    assert_eq!(
        tool["annotations"]["idempotentHint"], true,
        "a handle names two published versions, which are immutable, so the \
         same page is the same page, got {}",
        tool["annotations"]
    );
}

#[tokio::test]
async fn the_handle_this_tool_takes_is_described_by_the_module_that_mints_it() {
    let tool = listed(TOOL).await;
    let handle = &tool["inputSchema"]["properties"]["handle"];

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
async fn the_path_this_tool_takes_is_described_the_way_a_prefix_is() {
    let path = listed(TOOL).await["inputSchema"]["properties"]["path"].clone();
    let prefix = listed("list_package_files").await["inputSchema"]["properties"]["prefix"].clone();

    assert_eq!(
        path, prefix,
        "one rule, written once, shown by both tools that take it"
    );
    assert!(
        path["description"]
            .as_str()
            .is_some_and(|said| said.contains("`/`") && said.contains("empty string")),
        "the description says what `/` and `\"\"` ask for, got {path}"
    );
}

#[tokio::test]
async fn the_whole_tree_walks_to_the_totals_the_summary_reports() {
    let summary = call(
        SUMMARY,
        json!({
            "registry": "npm",
            "package": "diffable",
            "from_version": "1.0.0",
            "to_version": "2.0.0",
        }),
    )
    .await;

    let handle = summary["structuredContent"]["handle"]
        .as_str()
        .unwrap_or_else(|| panic!("the summary carries a handle, got {summary}"))
        .to_owned();

    let nodes = walk(json!({ "handle": handle })).await;

    let mut counted = json!({
        "added": 0, "removed": 0, "modified": 0, "renamed": 0, "unchanged": 0,
        "lines_added": 0, "lines_removed": 0,
    });
    for node in nodes.iter().filter(|node| node["type"] == "file") {
        let status = node["status"].as_str().expect("every node has a status");
        counted[status] = json!(counted[status].as_u64().unwrap_or(0) + 1);
        for (field, counted_as) in [
            ("lines_added", "lines_added"),
            ("lines_removed", "lines_removed"),
        ] {
            let moved = node[field].as_u64().unwrap_or(0);
            counted[counted_as] = json!(counted[counted_as].as_u64().unwrap_or(0) + moved);
        }
    }

    assert_eq!(
        counted, summary["structuredContent"]["totals"],
        "walking the tree completely should count what the summary counted: \
         got {counted} against {summary}"
    );
}

#[tokio::test]
async fn a_node_carries_its_path_its_kind_its_status_and_what_moved() {
    let nodes = walk(json!({ "handle": diffable() })).await;

    assert_eq!(
        nodes,
        vec![
            json!({
                "path": "README.md",
                "type": "file",
                "status": "unchanged",
                "lines_added": 0,
                "lines_removed": 0,
            }),
            json!({
                "path": "src",
                "type": "directory",
                "status": "modified",
                "lines_added": 2,
                "lines_removed": 2,
            }),
            json!({
                "path": "src/added.js",
                "type": "file",
                "status": "added",
                "lines_added": 1,
                "lines_removed": 0,
            }),
            json!({
                "path": "src/index.js",
                "type": "file",
                "status": "modified",
                "lines_added": 1,
                "lines_removed": 1,
            }),
            json!({
                "path": "src/new-name.js",
                "type": "file",
                "status": "renamed",
                "lines_added": 0,
                "lines_removed": 0,
                "old_path": "src/old-name.js",
            }),
            json!({
                "path": "src/removed.js",
                "type": "file",
                "status": "removed",
                "lines_added": 0,
                "lines_removed": 1,
            }),
        ],
        "a directory comes before what is under it and the paths sort, which \
         is the order the comparison itself is in"
    );
}

#[tokio::test]
async fn every_node_appears_exactly_once_across_the_pages() {
    let narrowings = [
        json!({}),
        json!({ "path": "src" }),
        json!({ "depth": 1 }),
        json!({ "status": ["added", "removed", "renamed"] }),
        json!({ "path": "src", "depth": 1, "status": ["added", "renamed"] }),
    ];

    for narrowing in narrowings {
        let mut arguments = json!({ "handle": diffable() });
        for (field, value) in narrowing.as_object().expect("an object of arguments") {
            arguments[field] = value.clone();
        }

        let whole = walk(arguments.clone()).await;
        assert!(
            !whole.is_empty(),
            "{arguments} should select something, or the page sizes below \
             agree about nothing"
        );

        for limit in [1, 2, 5] {
            let mut asked = arguments.clone();
            asked["limit"] = json!(limit);

            assert_eq!(
                walk(asked).await,
                whole,
                "a walk at {limit} to a page should cover the same nodes in \
                 the same order as one that took them all at once, and {arguments} \
                 is what it was narrowed to"
            );
        }
    }
}

#[tokio::test]
async fn a_page_reports_the_whole_comparisons_length() {
    let first = call(TOOL, json!({ "handle": diffable(), "limit": 2 })).await;
    let page = &first["structuredContent"];

    assert_eq!(
        page["items"].as_array().map(Vec::len),
        Some(2),
        "got {first}"
    );
    assert_eq!(
        page["total"],
        json!(6),
        "the total is the comparison's length and not the page's, which is \
         what tells an agent there is more: got {first}"
    );
    assert!(
        page["nextCursor"].is_string(),
        "and there is a cursor to follow, got {first}"
    );
}

#[tokio::test]
async fn a_path_returns_that_subtree_and_nothing_outside_it() {
    let nodes = walk(json!({ "handle": diffable(), "path": "src" })).await;

    assert_eq!(
        paths(&nodes),
        [
            "src/added.js",
            "src/index.js",
            "src/new-name.js",
            "src/removed.js",
        ],
        "`README.md` is outside `src` and `src` is not inside itself"
    );
}

#[tokio::test]
async fn a_subtree_deeper_down_is_reached_by_naming_it() {
    let nodes = walk(json!({
        "handle": handle("many-files", "1.0.0", "1.0.0"),
        "path": "src/07",
    }))
    .await;

    assert_eq!(nodes.len(), 100, "one directory's hundred files");
    let strays: Vec<&str> = paths(&nodes)
        .into_iter()
        .filter(|path| !path.starts_with("src/07/"))
        .collect();
    assert!(strays.is_empty(), "nothing outside the subtree: {strays:?}");
}

#[tokio::test]
async fn a_trailing_slash_on_a_path_makes_no_difference() {
    let bare = walk(json!({ "handle": diffable(), "path": "src" })).await;
    let slashed = walk(json!({ "handle": diffable(), "path": "src/" })).await;

    assert_eq!(bare, slashed);
}

#[tokio::test]
async fn a_path_of_a_slash_is_the_whole_comparison() {
    let whole = walk(json!({ "handle": diffable() })).await;
    let rooted = walk(json!({ "handle": diffable(), "path": "/" })).await;

    assert!(
        paths(&whole).contains(&"README.md") && paths(&whole).contains(&"src"),
        "the control: the whole comparison has the top level in it, got {:?}",
        paths(&whole)
    );
    assert_eq!(
        rooted, whole,
        "`/` is the root, and its subtree is everything"
    );
}

#[tokio::test]
async fn a_null_path_is_the_whole_comparison() {
    let whole = walk(json!({ "handle": diffable() })).await;
    let null = walk(json!({ "handle": diffable(), "path": null })).await;

    assert!(
        !whole.is_empty(),
        "the control: a comparison with nodes in it"
    );
    assert_eq!(null, whole, "`null` is the argument left out");
}

#[tokio::test]
async fn an_empty_path_is_the_whole_comparison() {
    let whole = walk(json!({ "handle": diffable() })).await;
    let empty = walk(json!({ "handle": diffable(), "path": "" })).await;

    assert!(
        !whole.is_empty(),
        "the control: a comparison with nodes in it"
    );
    assert_eq!(
        empty, whole,
        "`\"\"` is the root, not a directory named nothing"
    );
}

#[tokio::test]
async fn a_path_is_matched_at_the_separator_and_not_by_its_characters() {
    let wheel = DiffHandle::mint(Inputs {
        registry: Registry::PyPi,
        package: "tensorflow".to_owned(),
        from_version: "2.16.1".to_owned(),
        to_version: "2.16.1".to_owned(),
        similarity_threshold: 0.75,
        ignore_whitespace: false,
    })
    .encode();

    let shadowed = walk(json!({ "handle": &wheel, "path": "tensorflow-2.16.1.dist-info" })).await;
    assert_eq!(
        paths(&shadowed),
        ["tensorflow-2.16.1.dist-info/METADATA"],
        "`tensorflow` begins this path and is not this path, so the descent \
         has to pass it by rather than turn into it and come back empty"
    );

    let deep = handle("many-files", "1.0.0", "1.0.0");
    for path in ["sr", "src/0", "src/070"] {
        let result = call(TOOL, json!({ "handle": &deep, "path": path })).await;

        assert_eq!(
            result["structuredContent"]["total"],
            json!(0),
            "`{path}` is not a directory of this package's, and a match on \
             characters rather than on path components would have answered \
             with one that is: got {result}"
        );
    }

    let partial = call(TOOL, json!({ "handle": diffable(), "path": "src/index" })).await;
    assert_eq!(
        partial["structuredContent"]["total"],
        json!(0),
        "got {partial}"
    );
}

#[tokio::test]
async fn a_path_with_nothing_under_it_is_an_empty_page() {
    for path in ["src/index.js", "does-not-exist"] {
        let result = call(TOOL, json!({ "handle": diffable(), "path": path })).await;

        assert_eq!(
            result["isError"],
            json!(false),
            "asking about an empty corner of a comparison is not a failure, \
             got {result}"
        );
        assert_eq!(
            result["structuredContent"]["items"],
            json!([]),
            "got {result}"
        );
        assert_eq!(
            result["structuredContent"]["total"],
            json!(0),
            "and the total says so rather than the page being short: got {result}"
        );
    }
}

#[tokio::test]
async fn a_depth_of_one_returns_only_the_immediate_children() {
    let nodes = walk(json!({ "handle": diffable(), "depth": 1 })).await;

    assert_eq!(
        paths(&nodes),
        ["README.md", "src"],
        "`src` is listed and what is inside it is not"
    );
}

#[tokio::test]
async fn depth_is_counted_from_the_path_the_listing_was_rooted_at() {
    let nodes = walk(json!({
        "handle": handle("many-files", "1.0.0", "1.0.0"),
        "path": "src",
        "depth": 1,
    }))
    .await;

    assert_eq!(
        nodes.len(),
        26,
        "the directories `src` holds, and none of the files inside them"
    );
    assert!(
        nodes.iter().all(|node| node["type"] == "directory"),
        "got {nodes:?}"
    );
}

#[tokio::test]
async fn a_deeper_depth_takes_in_one_more_level() {
    let handle = handle("many-files", "1.0.0", "1.0.0");

    let top = walk(json!({ "handle": handle, "depth": 1 })).await;
    assert_eq!(paths(&top), ["src"], "one directory at the top");

    let next = walk(json!({ "handle": handle, "depth": 2 })).await;
    assert_eq!(
        next.len(),
        27,
        "`src` and the twenty-six directories under it, still no files"
    );
}

#[tokio::test]
async fn an_absent_depth_descends_the_whole_way() {
    let nodes = walk(json!({ "handle": diffable() })).await;

    assert!(
        paths(&nodes).contains(&"src/index.js"),
        "a file two levels down is in an answer nobody limited: got {nodes:?}"
    );
}

#[tokio::test]
async fn a_depth_below_one_is_read_as_one() {
    let nodes = walk(json!({ "handle": diffable(), "depth": 0 })).await;

    assert_eq!(paths(&nodes), ["README.md", "src"]);
}

#[tokio::test]
async fn the_statuses_an_agent_may_filter_by_are_the_ones_a_node_carries() {
    let tool = listed(TOOL).await;
    let schema = tool["inputSchema"].clone();

    let asked = enumeration(&schema, &schema["properties"]["status"]["items"]);
    assert_eq!(
        asked,
        Some(json!([
            "added",
            "removed",
            "modified",
            "renamed",
            "unchanged"
        ])),
        "got {schema}"
    );

    let output = tool["outputSchema"].clone();
    let carried = enumeration(&output, &output["$defs"]["Node"]["properties"]["status"]);
    assert_eq!(carried, asked, "got {output}");
}

#[tokio::test]
async fn filtering_by_status_returns_exactly_the_matching_nodes() {
    let nodes = walk(json!({ "handle": diffable(), "status": ["added"] })).await;

    assert_eq!(paths(&nodes), ["src/added.js"]);
}

#[tokio::test]
async fn several_statuses_can_be_asked_for_at_once() {
    let nodes = walk(json!({
        "handle": diffable(),
        "status": ["added", "removed", "renamed"],
    }))
    .await;

    assert_eq!(
        paths(&nodes),
        ["src/added.js", "src/new-name.js", "src/removed.js"],
        "three statuses, in the order the comparison is in rather than the \
         order they were asked for"
    );
}

#[tokio::test]
async fn a_directory_is_filtered_on_the_status_that_summarises_it() {
    let nodes = walk(json!({ "handle": diffable(), "status": ["modified"] })).await;

    assert_eq!(
        paths(&nodes),
        ["src", "src/index.js"],
        "one file changed and one directory reports that something under it \
         did"
    );
}

#[tokio::test]
async fn a_directory_only_one_version_has_carries_that_rather_than_modified() {
    let handle = handle("moved", "2.0.0", "1.0.0");
    let nodes = walk(json!({ "handle": handle })).await;

    let statuses: Vec<(&str, &str)> = nodes
        .iter()
        .map(|node| {
            (
                node["path"].as_str().expect("every node has a path"),
                node["status"].as_str().expect("every node has a status"),
            )
        })
        .collect();

    assert_eq!(
        statuses,
        [
            ("README.md", "unchanged"),
            ("src", "modified"),
            ("src/legacy", "added"),
            ("src/legacy/reporter.js", "renamed"),
        ],
        "`src/legacy` is new in the second version and is `added`, not \
         `modified`, although what is under it is a rename"
    );

    let modified = walk(json!({ "handle": handle, "status": ["modified"] })).await;
    assert_eq!(
        paths(&modified),
        ["src"],
        "so asking for `modified` alone leaves out the directory that \
         appeared and everything in it, which is what the argument's \
         description has to say rather than leave to be found"
    );
}

#[tokio::test]
async fn an_empty_list_of_statuses_narrows_nothing() {
    let filtered = walk(json!({ "handle": diffable(), "status": [] })).await;
    let whole = walk(json!({ "handle": diffable() })).await;

    assert_eq!(filtered, whole);
}

#[tokio::test]
async fn the_description_says_unchanged_is_rarely_what_is_wanted() {
    let tool = listed(TOOL).await;
    let said = tool["description"]
        .as_str()
        .unwrap_or_else(|| panic!("a described tool, got {tool}"));

    assert!(
        said.contains("status"),
        "the filter is the difference between a page of what changed and a \
         page of what did not: {said}"
    );
    assert!(
        said.contains("unchanged"),
        "and the status worth excluding is named: {said}"
    );
}

#[tokio::test]
async fn a_directorys_counts_are_the_sum_of_its_childrens() {
    for package in ["diffable", "churny", "moved"] {
        let nodes = walk(json!({ "handle": handle(package, "1.0.0", "2.0.0") })).await;

        let mut summed: HashMap<String, (u64, u64)> = HashMap::new();
        for node in &nodes {
            let path = node["path"].as_str().expect("every node has a path");
            let parent = match path.rsplit_once('/') {
                Some((parent, _)) => parent.to_owned(),
                None => continue,
            };
            let counts = summed.entry(parent).or_default();
            counts.0 += node["lines_added"].as_u64().unwrap_or(0);
            counts.1 += node["lines_removed"].as_u64().unwrap_or(0);
        }

        for directory in nodes.iter().filter(|node| node["type"] == "directory") {
            let path = directory["path"].as_str().expect("every node has a path");
            let (added, removed) = summed.get(path).copied().unwrap_or_default();

            assert_eq!(
                (
                    directory["lines_added"].as_u64(),
                    directory["lines_removed"].as_u64()
                ),
                (Some(added), Some(removed)),
                "`{path}` in `{package}` should carry what is under it: got {directory}"
            );
        }
    }
}

#[tokio::test]
async fn a_directory_a_rename_emptied_is_not_in_the_tree_at_all() {
    let handle = handle("moved", "1.0.0", "2.0.0");
    let nodes = walk(json!({ "handle": handle })).await;

    assert_eq!(
        paths(&nodes),
        ["README.md", "src", "src/reporter.js"],
        "`src/legacy` held one file and the file left"
    );

    let moved = &nodes[2];
    assert_eq!(moved["status"], json!("renamed"), "got {moved}");
    assert_eq!(
        moved["old_path"],
        json!("src/legacy/reporter.js"),
        "where it came from is the half an agent cannot work out from where \
         it is now: got {moved}"
    );

    let gone = call(TOOL, json!({ "handle": handle, "path": "src/legacy" })).await;
    assert_eq!(
        gone["structuredContent"]["total"],
        json!(0),
        "a directory the comparison does not have is empty rather than an \
         error: got {gone}"
    );
    assert_eq!(gone["isError"], json!(false), "got {gone}");
}

#[tokio::test]
async fn a_path_that_is_a_file_in_one_version_and_a_directory_in_the_other_is_two_nodes() {
    for (from, to, expected) in [
        (
            "1.0.0",
            "2.0.0",
            [
                ("lib", "file", "removed"),
                ("lib", "directory", "added"),
                ("lib/index.js", "file", "added"),
                ("package.json", "file", "unchanged"),
            ],
        ),
        (
            "2.0.0",
            "1.0.0",
            [
                ("lib", "directory", "removed"),
                ("lib/index.js", "file", "removed"),
                ("lib", "file", "added"),
                ("package.json", "file", "unchanged"),
            ],
        ),
    ] {
        let nodes = walk(json!({ "handle": handle("shape", from, to) })).await;

        let listed: Vec<(&str, &str, &str)> = nodes
            .iter()
            .map(|node| {
                (
                    node["path"].as_str().unwrap_or_default(),
                    node["type"].as_str().unwrap_or_default(),
                    node["status"].as_str().unwrap_or_default(),
                )
            })
            .collect();
        assert_eq!(listed, expected, "{from} → {to}: got {nodes:?}");
    }
}

#[tokio::test]
async fn a_path_that_is_a_file_and_a_directory_lists_the_directorys_contents() {
    for (from, to) in [("1.0.0", "2.0.0"), ("2.0.0", "1.0.0")] {
        let nodes = walk(json!({ "handle": handle("shape", from, to), "path": "lib" })).await;

        assert_eq!(
            paths(&nodes),
            ["lib/index.js"],
            "{from} → {to}: `lib` is a directory in 2.0.0, holding one file"
        );
    }
}

#[tokio::test]
async fn the_description_says_a_path_can_be_a_file_and_a_directory() {
    let tool = listed(TOOL).await;
    let said = tool["description"]
        .as_str()
        .unwrap_or_else(|| panic!("a described tool, got {tool}"));

    assert!(
        said.contains("listed twice"),
        "a path that is a file in one version and a directory in the other \
         is two nodes: {said}"
    );
}

#[tokio::test]
async fn the_description_says_a_directory_a_rename_emptied_is_not_there() {
    let tool = listed(TOOL).await;
    let said = tool["description"]
        .as_str()
        .unwrap_or_else(|| panic!("a described tool, got {tool}"));

    assert!(
        said.contains("rename") && said.contains("empty page"),
        "a directory the first version had can be missing from the \
         comparison, and asking for it is an empty page: {said}"
    );
}

#[tokio::test]
async fn no_page_of_the_largest_pair_here_is_over_the_response_ceiling() {
    let pages = pages(json!({
        "handle": handle("many-files", "1.0.0", "1.0.0"),
        "limit": 1_000,
    }))
    .await;

    assert!(
        pages.len() > 1,
        "the largest pair here should take more than one page at the largest \
         limit, or this proves nothing about a walk"
    );

    for (body, result) in &pages {
        assert!(
            body.len() < page::RESPONSE_CEILING,
            "a page of {} nodes came back as {} bytes, and the platform \
             refuses anything over {}",
            result["structuredContent"]["items"]
                .as_array()
                .map_or(0, Vec::len),
            body.len(),
            page::RESPONSE_CEILING,
        );
    }
}

#[tokio::test]
async fn a_handle_nothing_has_computed_before_is_answered_anyway() {
    let result = call(TOOL, json!({ "handle": diffable() })).await;

    assert_eq!(
        result["isError"],
        json!(false),
        "a comparison nobody has made yet is one this server can make: got {result}"
    );
    assert_eq!(
        result["structuredContent"]["total"],
        json!(6),
        "got {result}"
    );
}

#[tokio::test]
async fn the_same_handle_answers_with_the_same_bytes_twice() {
    let arguments = json!({ "handle": diffable(), "limit": 2 });

    let first: Vec<String> = pages(arguments.clone())
        .await
        .into_iter()
        .map(|(body, _)| body)
        .collect();
    let again: Vec<String> = pages(arguments)
        .await
        .into_iter()
        .map(|(body, _)| body)
        .collect();

    assert_eq!(
        first, again,
        "the same handle, recomputed, should answer byte for byte as it did \
         before"
    );
}

#[tokio::test]
async fn a_handle_that_is_not_one_is_a_protocol_error() {
    let answer = Client::fixture()
        .post(json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "tools/call",
            "params": {
                "name": TOOL,
                "arguments": { "handle": "not-a-handle" },
            },
        }))
        .await;

    assert_eq!(answer["error"]["code"], -32602, "got {answer}");
    assert!(
        answer["result"]["isError"].is_null(),
        "a call that never ran is not a tool that failed, got {answer}"
    );
}

#[tokio::test]
async fn a_handle_whose_halves_disagree_is_a_protocol_error() {
    let answer = Client::fixture()
        .post(json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "tools/call",
            "params": {
                "name": TOOL,
                "arguments": { "handle": edited(&diffable(), "package", json!("elsewhere")) },
            },
        }))
        .await;

    assert_eq!(answer["error"]["code"], -32602, "got {answer}");
}

#[tokio::test]
async fn a_version_the_registry_does_not_have_names_what_was_not_found() {
    let result = call(
        TOOL,
        json!({ "handle": handle("diffable", "1.0.0", "9.9.9") }),
    )
    .await;

    assert_eq!(result["isError"], json!(true), "got {result}");

    let said = result["content"][0]["text"]
        .as_str()
        .unwrap_or_else(|| panic!("a tool error carries text a model reads, got {result}"));
    assert!(
        said.contains("9.9.9") && said.contains("diffable"),
        "the refusal names what was not found: got {said}"
    );
}

#[tokio::test]
async fn a_package_the_registry_does_not_have_names_what_was_not_found() {
    let handle = DiffHandle::mint(Inputs {
        registry: Registry::Crates,
        package: "not-a-real-crate".to_owned(),
        from_version: "1.0.0".to_owned(),
        to_version: "1.0.0".to_owned(),
        similarity_threshold: 0.75,
        ignore_whitespace: false,
    })
    .encode();

    let result = call(TOOL, json!({ "handle": handle })).await;

    assert_eq!(result["isError"], json!(true), "got {result}");

    let said = result["content"][0]["text"]
        .as_str()
        .unwrap_or_else(|| panic!("a tool error carries text a model reads, got {result}"));
    assert!(
        said.contains("not-a-real-crate"),
        "the refusal names the package rather than only saying no: got {said}"
    );
}

fn edited(handle: &str, field: &str, value: Value) -> String {
    let encoded = handle
        .strip_prefix("d1:")
        .expect("a handle this server minted");
    let payload = URL_SAFE_NO_PAD
        .decode(encoded)
        .expect("a handle's payload is base64url");
    let mut payload: Value = serde_json::from_slice(&payload).expect("a handle's payload is JSON");

    payload[field] = value;

    format!("d1:{}", URL_SAFE_NO_PAD.encode(payload.to_string()))
}

fn diffable() -> String {
    handle("diffable", "1.0.0", "2.0.0")
}

fn handle(package: &str, from: &str, to: &str) -> String {
    DiffHandle::mint(Inputs {
        registry: Registry::Npm,
        package: package.to_owned(),
        from_version: from.to_owned(),
        to_version: to.to_owned(),
        similarity_threshold: 0.75,
        ignore_whitespace: false,
    })
    .encode()
}

fn enumeration(root: &Value, field: &Value) -> Option<Value> {
    let named = match field["$ref"].as_str() {
        Some(reference) => {
            let name = reference.rsplit('/').next()?;
            root["$defs"][name].clone()
        }
        None => field.clone(),
    };

    named["enum"].as_array().cloned().map(Value::Array)
}

fn paths(nodes: &[Value]) -> Vec<&str> {
    nodes
        .iter()
        .map(|node| {
            node["path"]
                .as_str()
                .unwrap_or_else(|| panic!("every node has a path, got {node}"))
        })
        .collect()
}

async fn walk(arguments: Value) -> Vec<Value> {
    pages(arguments)
        .await
        .iter()
        .flat_map(|(_, result)| {
            result["structuredContent"]["items"]
                .as_array()
                .cloned()
                .unwrap_or_default()
        })
        .collect()
}

async fn pages(arguments: Value) -> Vec<(String, Value)> {
    let mut collected: Vec<(String, Value)> = Vec::new();
    let mut seen = 0;
    let mut cursor: Option<String> = None;

    loop {
        let mut asked = arguments.clone();
        if let Some(cursor) = &cursor {
            asked["cursor"] = json!(cursor);
        }

        let answer = Client::fixture()
            .respond(json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "tools/call",
                "params": { "name": TOOL, "arguments": asked },
            }))
            .await;

        let result = answer.result();
        let page = result["structuredContent"].clone();

        seen += page["items"]
            .as_array()
            .unwrap_or_else(|| panic!("a page carries items, got {result}"))
            .len();
        collected.push((answer.frame(), result));

        match page["nextCursor"].as_str() {
            Some(next) => cursor = Some(next.to_owned()),
            None => {
                assert_eq!(
                    page["total"],
                    json!(seen),
                    "a walk that followed every cursor should have every node \
                     the last page said there were: got {page}"
                );
                return collected;
            }
        }
    }
}

async fn listed(name: &str) -> Value {
    Client::fixture().listed(name).await
}

async fn call(tool: &str, arguments: Value) -> Value {
    Client::fixture().call(tool, arguments).await
}
