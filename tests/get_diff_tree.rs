//! `get_diff_tree`, driven the way an agent drives it.
//!
//! One seam: the wire. `tools/list` for what a client is told and
//! `tools/call` for what it gets back, both through `router_with` over the
//! fixture archives. Testing here rather than at the handler is what keeps
//! the definition and the handler from drifting apart: a schema that stopped
//! describing what the handler reads is a confident wrong answer to a model,
//! and a handler test would go on passing through it.
//!
//! The handle this tool takes comes from `diff_package_versions` wherever one
//! test can get it from there, because the two tools' agreement is the thing
//! worth asserting: a tree that walked to different totals than the summary
//! beside it would make an agent's second call contradict its first.
//!
//! # What is deliberately not asserted here
//!
//! That a walk of a sequence is stable under the response ceiling, and that a
//! cursor of a client's own invention is refused. Those are `src/page.rs`'s
//! and `tests/page.rs` holds them against a generated sequence. What this
//! suite asserts is that this tool goes *through* that module rather than
//! around it — that every node of one comparison is reachable by following
//! the cursors, whatever the page size.
//!
//! That the handle format is what it is. `tests/handle.rs` holds the four
//! ways a handle is refused against hand-built payloads, which is a stronger
//! fixture than any package pair. What is here is the two of them reaching
//! *this* tool's argument, because a tool that took a handle and verified it
//! late would pass every test in that file.
//!
//! That the answer carries `ttlMs` and `cacheScope`. #14 asks for both and a
//! `tools/call` result has nowhere to put them: in the `2026-07-28` schema
//! `CacheableResult` is extended by `DiscoverResult`, the four list results
//! and `ReadResourceResult`, while `CallToolResult` extends plain `Result`.
//! The freshness hint a diff wants is a resource's to carry, which is #16 —
//! the same answer #11, #19 and #42 arrived at before this.

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

/// The tool that mints what this one takes.
const SUMMARY: &str = "diff_package_versions";

// ---------------------------------------------------------------------------
// What a client is told
// ---------------------------------------------------------------------------

/// The arguments and the answer this tool in particular has, which is #23's
/// question asked of one tool.
///
/// The rules every tool is held to — a description, an object input schema, a
/// declared output shape, the read-only and open-world hints, the registry
/// enum — are `tests/tools.rs`'s, over all eight at once. What is here is
/// only what is true of this one.
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

/// The handle this tool takes is described by the module that mints it.
///
/// The same assertion `tests/diff_package_versions.rs` makes about the answer
/// that carries one, and it is here for the half that would break separately:
/// a doc comment on this argument would override the type's description, and
/// what an agent would lose is the sentence saying a handle is passed back
/// unchanged rather than built by hand — in the schema of the tool it is most
/// tempting to build one for.
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

/// `path` carries the one rule for a subtree, and `list_package_files`'
/// `prefix` carries the same one.
///
/// Two tools take a directory whose subtree is asked for, and when each wrote
/// its own description the two answered `/` differently (#97). So the schema
/// an agent reads for either argument is one schema, and it says what `/`
/// and `""` mean rather than leaving it to be guessed from "omit it".
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

// ---------------------------------------------------------------------------
// What it answers
// ---------------------------------------------------------------------------

/// The whole tree, walked, is the summary the other tool gave.
///
/// The strongest thing this tool can be held to, and the reason the handle
/// below comes from `diff_package_versions` rather than from this test: an
/// agent reads a summary, asks for the tree it describes, and the two have to
/// be the same comparison. A tree that counted differently would make the
/// second call contradict the first with nothing to say which was wrong.
///
/// Files only. The engine gives a directory the sum of what is under it, so
/// counting directories here would report every change once per directory
/// above it — which is what the summary's own totals are documented not to
/// do.
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

/// The nodes are the comparison's own, in the engine's vocabulary.
///
/// `diffable` 1.0.0 → 2.0.0 has one file of each status, which makes every
/// row below a fact about the fixture rather than about what this tool was
/// asked for. The directory is in the listing too: a tree is what this
/// answers with, and `src` is part of it.
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

/// Every node appears exactly once, however small the pages are, and
/// whatever the walk was narrowed to first.
///
/// The property a cursor is for. Several page sizes rather than one: a walk
/// that agrees with itself at one size and not another has a cursor that
/// names a position in an arrangement built for that request.
///
/// And each narrowing rather than the whole comparison alone, which is the
/// half that would break separately. A cursor is an offset into whatever
/// sequence this tool handed `page::paginate`, so `path`, `depth` and
/// `status` decide what that sequence *is* before a cursor names a position
/// in it. If any of the three produced a different sequence on the call that
/// resumes than on the call that started — a `path` resolved by walking in
/// some other order, a filter applied to a page rather than to the walk —
/// then a node would be served twice or not at all, and only at a page size
/// small enough to cross the seam. All three together as well, because they
/// compose and the composition is what an agent actually sends.
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

/// A page says how much there is, not how much it handed over.
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

// ---------------------------------------------------------------------------
// One subtree
// ---------------------------------------------------------------------------

/// `path` answers with what is under that directory and nothing else.
///
/// The directory itself is not in it, which is the half an agent notices: a
/// listing of `src` that began with `src` would make a walk of a subtree
/// disagree with the same nodes read out of a walk of the whole tree.
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

/// A directory several levels down is reachable by naming it.
///
/// `many-files` spreads two and a half thousand files over twenty-five
/// directories, so a subtree of it is a real narrowing rather than the whole
/// answer with one row missing — and the pair is the package against itself,
/// which is the only way an archive that exists at one version can be
/// compared at all.
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

/// A trailing slash is allowed and changes nothing.
///
/// An agent writing a directory as `src/` is writing the same directory, and
/// a tool that answered differently would be asking it to know which
/// spelling this server prefers.
#[tokio::test]
async fn a_trailing_slash_on_a_path_makes_no_difference() {
    let bare = walk(json!({ "handle": diffable(), "path": "src" })).await;
    let slashed = walk(json!({ "handle": diffable(), "path": "src/" })).await;

    assert_eq!(bare, slashed);
}

/// `/` is the root, and the root's subtree is the whole comparison.
///
/// What is left of `/` once its trailing slash is gone is nothing, and
/// nothing is the root. `list_package_files` once read the same argument as a
/// directory called `/` and answered an empty page, so this is pinned from
/// both tools rather than assumed of either.
#[tokio::test]
async fn a_path_of_a_slash_is_the_whole_comparison() {
    let whole = walk(json!({ "handle": diffable() })).await;
    let rooted = walk(json!({ "handle": diffable(), "path": "/" })).await;

    assert!(
        paths(&whole).contains(&"README.md") && paths(&whole).contains(&"src"),
        "the control: the whole comparison has the top level in it, got {:?}",
        paths(&whole)
    );
    assert_eq!(rooted, whole, "`/` is the root, not a directory named `/`");
}

/// An empty `path` is the root too.
///
/// Nothing is what a trailing slash leaves of `/`, so `""` and `/` are one
/// argument. An agent that builds a path by joining nothing to a slash should
/// not find the two disagree.
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

/// A path is matched at the separator, so one directory's name cannot be the
/// beginning of another's and swallow it.
///
/// The "and nothing outside it" half, and the one a string comparison gets
/// wrong. `path` is resolved by descending — at each level the walk follows
/// the one child the path lies inside — so a comparison that asked only
/// whether the path *begins with* a child's would take the wrong branch the
/// moment two siblings share a beginning, and then find nothing at the bottom
/// of it. A directory that exists would come back as an empty page.
///
/// TensorFlow's wheel is that package. Its two top-level directories are
/// `tensorflow` and `tensorflow-2.16.1.dist-info`, the first a strict prefix
/// of the second and sorted before it — the `src/legacy` against
/// `src/legacy-old` case, in a fixture the set already had. Asking for the
/// longer one has to answer with what is in it and not with a walk that went
/// into the shorter one and gave up.
///
/// The other direction below it: a name shorter than a real one, and a name
/// that is a real one with more on the end, neither of which is a directory.
/// The sibling of `list_package_files`' own test, against the tool that
/// documents the same rule.
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

    // And a file's path is not reached by a prefix of it either: `src/index`
    // names nothing, where `src/index.js` names a file with nothing under it.
    let partial = call(TOOL, json!({ "handle": diffable(), "path": "src/index" })).await;
    assert_eq!(
        partial["structuredContent"]["total"],
        json!(0),
        "got {partial}"
    );
}

/// A directory the comparison does not have is an empty page, not a refusal.
///
/// Two reasons rather than consistency with `list_package_files` alone. A
/// path that is a file names something real with nothing under it, and the
/// honest answer to "what is inside this" is nothing. And a directory the
/// engine pruned — because a rename took its last file away — is a directory
/// an agent can have read about in the first version and cannot see in the
/// comparison, which is the same answer for a different reason.
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

// ---------------------------------------------------------------------------
// How far down
// ---------------------------------------------------------------------------

/// `depth: 1` is the top of the comparison and nothing under it.
///
/// What an agent does first: a package it has never seen, asked what it is
/// made of before asking what changed inside any of it.
#[tokio::test]
async fn a_depth_of_one_returns_only_the_immediate_children() {
    let nodes = walk(json!({ "handle": diffable(), "depth": 1 })).await;

    assert_eq!(
        paths(&nodes),
        ["README.md", "src"],
        "`src` is listed and what is inside it is not"
    );
}

/// Depth is counted from wherever the listing is rooted, not from the
/// comparison's root.
///
/// The two arguments have to compose: an agent that narrowed to `src` and
/// asked for one level is asking about `src`'s children, and a depth counted
/// from the top would answer with nothing at all for anything deeper than
/// one.
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

/// Each level down is one more.
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

/// Omitting it descends as far as the comparison goes.
#[tokio::test]
async fn an_absent_depth_descends_the_whole_way() {
    let nodes = walk(json!({ "handle": diffable() })).await;

    assert!(
        paths(&nodes).contains(&"src/index.js"),
        "a file two levels down is in an answer nobody limited: got {nodes:?}"
    );
}

/// A depth below one is read as one rather than as nothing.
///
/// `0` is the value an agent arrives at by counting from zero, and the
/// answer it would otherwise get — an empty page — reads as "this directory
/// is empty", which is a different fact about the comparison. The same shape
/// as a limit being clamped into its range instead of refused.
#[tokio::test]
async fn a_depth_below_one_is_read_as_one() {
    let nodes = walk(json!({ "handle": diffable(), "depth": 0 })).await;

    assert_eq!(paths(&nodes), ["README.md", "src"]);
}

// ---------------------------------------------------------------------------
// Which statuses
// ---------------------------------------------------------------------------

/// The five an agent may ask for are the five a node can have.
///
/// A filter is only usable if the values in it are the values in the answer,
/// and this is the one place the two could part company: the status a node
/// carries and the status a caller passes are the same enumeration read in
/// two directions.
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

/// Asking for one status answers with exactly the nodes that have it.
#[tokio::test]
async fn filtering_by_status_returns_exactly_the_matching_nodes() {
    let nodes = walk(json!({ "handle": diffable(), "status": ["added"] })).await;

    assert_eq!(paths(&nodes), ["src/added.js"]);
}

/// And asking for several answers with all of them, still in tree order.
///
/// The combination is the case worth pinning: a filter implemented as a
/// comparison rather than as a set would answer the first status and drop the
/// rest, which reads as a comparison in which nothing was removed.
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

/// A directory is kept or dropped on its own status, which is a summary of
/// what is under it.
///
/// Worth its own test because it is the one part of this filter that can
/// surprise: `src` is `modified` in a comparison where `src` itself did not
/// move, because something inside it did. An agent that wants files alone has
/// the `type` on every node to say so, and this is the behaviour that makes
/// that necessary — so it is asserted rather than left to be discovered.
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

/// A directory the second version added is `added`, whatever moved into it.
///
/// The half of the rule above that a summary reading of it gets wrong. A
/// directory's status is the directory's own — the engine asks whether each
/// version has it before it asks what happened underneath — so a directory
/// only one of the two versions has is `added` or `removed` and never
/// `modified`, however much moved in or out of it.
///
/// `moved` compared backwards is the smallest case: `src/reporter.js`
/// becomes `src/legacy/reporter.js`, so `src/legacy` is a directory the
/// second version has and the first does not, holding one `renamed` file and
/// nothing else. It matters to a caller rather than only to a reader of the
/// engine: an agent that asked for `modified` to find everywhere something
/// happened is answered with `src` alone here, and neither the new directory
/// nor the file that moved into it is in that answer.
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

/// Naming no status narrows nothing.
///
/// An agent that built the argument from an empty list of interesting
/// statuses has asked for the whole comparison, which is what it would have
/// got by leaving the argument out. The alternative reading — nothing matches
/// — is an empty page that looks like a comparison in which nothing happened.
#[tokio::test]
async fn an_empty_list_of_statuses_narrows_nothing() {
    let filtered = walk(json!({ "handle": diffable(), "status": [] })).await;
    let whole = walk(json!({ "handle": diffable() })).await;

    assert_eq!(filtered, whole);
}

/// The description says what an agent should ask for, not only what it may.
///
/// `unchanged` is most of a version bump — the fixtures here are built to
/// have one file of each status, and a real package has thousands of files
/// that did not move — so an agent that does not know to exclude it spends
/// its pages on the part of the comparison it is not reading.
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

// ---------------------------------------------------------------------------
// The two things the engine does that an agent has to be told
// ---------------------------------------------------------------------------

/// A directory's counts are the sum of what is under it.
///
/// Held over every directory of two comparisons rather than one worked
/// example, because the property is what an agent needs in order to read the
/// numbers at all: adding a directory's lines to its files' lines counts the
/// same change twice, which is the mistake this makes possible and the
/// description warns about.
///
/// The sum is over immediate children, which is the same thing recursively:
/// each child already carries its own subtree's total.
#[tokio::test]
async fn a_directorys_counts_are_the_sum_of_its_childrens() {
    for package in ["diffable", "churny", "moved"] {
        let nodes = walk(json!({ "handle": handle(package, "1.0.0", "2.0.0") })).await;

        let mut summed: HashMap<String, (u64, u64)> = HashMap::new();
        for node in &nodes {
            let path = node["path"].as_str().expect("every node has a path");
            let parent = match path.rsplit_once('/') {
                Some((parent, _)) => parent.to_owned(),
                // The comparison's root is not listed, so a top-level node's
                // parent is nothing this walk will check.
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

/// A directory a rename emptied is gone from the comparison entirely.
///
/// `moved` is one file and one directory: `src/legacy/reporter.js` becomes
/// `src/reporter.js`, so nothing is left in `src/legacy` and the engine does
/// not list an empty directory. An agent reading the first version's files
/// and then this comparison would otherwise look for a directory that is
/// simply not there, which is why it is asserted rather than left as a
/// surprise — and why asking for that path answers with an empty page rather
/// than a refusal.
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

/// The description says so, because the argument's own description cannot.
///
/// `path` carries the rule every tool that takes a directory shares, and a
/// directory a rename emptied is this tool's alone: `list_package_files`
/// lists one version, where a directory is there or it is not. An agent that
/// read `src/legacy` in the first version and then asks for it here should
/// know before it asks that an empty page is the answer, not a sign of a
/// broken call.
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

// ---------------------------------------------------------------------------
// The ceiling
// ---------------------------------------------------------------------------

/// No answer to the largest pair here is one the platform would refuse.
///
/// Vercel drops a response over 4.5 MB rather than shortening it, so the
/// measurement is on the body a client received and not on the structure
/// inside it. `many-files` against itself is the biggest comparison this set
/// can produce — two and a half thousand files, every one of them a node —
/// and it is asked for at the largest page a caller can name.
///
/// What this does not prove is that the cut itself is correct: the fixture
/// is nowhere near the ceiling, because an archive that was would have to be
/// checked in. `tests/page.rs` holds the cut against a generated sequence,
/// and what is asserted here is that this tool's answers go through it.
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

// ---------------------------------------------------------------------------
// The handle, and the comparison behind it
// ---------------------------------------------------------------------------

/// A handle nothing has computed is answered, not refused.
///
/// This is the eviction case, and it is the only case there is today: each
/// request here is served by a server built for it, with no cache behind it
/// and nothing kept between calls, so the handle below names a comparison
/// this process has never made. ADR 0006 is the reason it can be answered at
/// all — the inputs travel with the `diff_id`, so a miss costs a
/// recomputation instead of an apology.
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

/// And the recomputed answer is the bytes the first one was.
///
/// The property that makes eviction invisible. If a walk after a miss
/// differed from a walk before it — in an order, in a count, in a cursor —
/// then an agent that asked twice would see a comparison change under it,
/// and the cache would be part of the answer rather than an optimisation.
///
/// The whole walk, and the bytes rather than the parsed structure: two
/// answers that differ only in the order of their fields are two answers.
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

/// A string that is not a handle never reaches the handler.
///
/// `-32602`, the protocol channel: a handle is minted and passed back, so
/// one that does not decode is something a client built and a model cannot
/// act on. The handler has no check of its own — the argument is read before
/// it runs — which is exactly what this asserts.
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

/// Nor does one whose two halves disagree.
///
/// The edited handle below names one comparison and describes another, which
/// is what an agent does when it changes the package and keeps the
/// identifier because the identifier looks like the opaque part. Answering it
/// would mean answering confidently about a comparison nobody asked for.
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

/// A version the registry does not have is an error the model sees.
///
/// A result carrying `isError` rather than a JSON-RPC error: the handle
/// decoded and the tool ran, and what failed is a fetch the model can correct
/// by comparing versions that exist. It names the package and the version,
/// because "not found" with nothing in it sends an agent to guess which half
/// of the comparison was wrong.
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

/// The same for a package that is not there at all.
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

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// `handle` with one field of its payload replaced.
///
/// What an agent does to a handle it can read part of: change the thing it is
/// asking about and leave the identifier, which looks like an internal
/// detail. The result is a handle whose `diff_id` no longer names the inputs
/// beside it — and the encoding is spelled out here rather than taken from
/// the crate, so this is a forgery a client could send rather than one this
/// server helped build.
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

/// A handle for `diffable` 1.0.0 → 2.0.0, minted rather than fetched.
///
/// The tool that mints one is called where the *agreement* between the two is
/// what is being asserted. Everywhere else a handle is just the argument, and
/// minting it here is one call rather than two.
fn diffable() -> String {
    handle("diffable", "1.0.0", "2.0.0")
}

/// A handle for one npm comparison, at the defaults `diff_package_versions`
/// would have used.
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

/// The values `field` allows, following a reference into `root`'s own
/// definitions when that is how the schema is written.
///
/// Which of the two `schemars` produces is its business and can change with
/// an upgrade; what the test is about is the list a reader ends up with.
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

/// The paths of `nodes`, in the order they came back.
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

/// Every node `arguments` selects, by following the cursors to the end.
///
/// What an agent does with a paginated answer, and the only way to ask this
/// tool for a whole comparison. The page's own `total` is checked against
/// what came back, so a walk that stopped early fails here rather than in
/// whichever assertion happened to read the last node.
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

/// The same walk, page by page, with the bytes each one arrived as.
///
/// The body beside the parsed result rather than instead of it, because two
/// tests ask about the bytes themselves: what the response ceiling bounds is
/// the frame this server wrote, and "the same answer twice" is a claim about
/// that frame rather than about a structure two parses happen to agree on.
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

/// The listed definition of `name`, or a panic naming what was listed.
async fn listed(name: &str) -> Value {
    Client::fixture().listed(name).await
}

/// Call `tool` with `arguments`, returning the `result`.
///
/// `tool` rather than this file's, because half of what is asserted here
/// starts at the tool that mints a handle.
async fn call(tool: &str, arguments: Value) -> Value {
    Client::fixture().call(tool, arguments).await
}
