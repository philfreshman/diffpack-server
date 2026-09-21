//! `diff_package_versions`, driven the way an agent drives it.
//!
//! One seam: the wire. `tools/list` for what a client is told and
//! `tools/call` for what it gets back, both through `router_with` over the
//! fixture archives. Everything this tool promises is reachable there —
//! including the failure, which is a result carrying `isError` rather than a
//! JSON-RPC error, and including the handle, which is a string in the answer
//! that `DiffHandle::decode` reads back.
//!
//! Testing here rather than at the handler is what keeps the definition and
//! the handler from drifting apart: a schema that stopped describing what the
//! handler reads is a confident wrong answer to a model, and a handler test
//! would go on passing through it.
//!
//! What is deliberately not re-proven here: that a `DiffKey` hashes to the
//! `diff_id` `docs/cache-key.md` fixes, and that a handle whose halves
//! disagree is refused. Those are `tests/cache_key.rs`'s and
//! `tests/handle.rs`'s, against the golden vectors and against hand-built
//! payloads, which are stronger fixtures than any package pair. What this
//! suite asserts is that this tool goes *through* those modules rather than
//! around them.
//!
//! Nor that no `description` an agent reads names a Rust path — `tests/tools.rs`
//! holds that over every tool, so a copy here would go on passing on the day
//! a new tool leaked one.
//!
//! The fixture pairs are built for the questions below and nothing else, which
//! is why they are tiny and synthetic: a real package pair makes every count
//! a number someone has to trust, and the acceptance criterion about parity
//! with `diffpack.io` is a fact about real registries that only a networked
//! run can state.

use axum::body::Body;
use axum::http::Request;
use diffpack_server::archive::Archive;
use diffpack_server::handle::{DiffHandle, Inputs};
use diffpack_server::mcp::Diffpack;
use diffpack_server::registry::Registry;
use diffpack_server::router;
use diffpack_server::tools::Ctx;
use http_body_util::BodyExt;
use serde_json::{json, Value};
use tower::ServiceExt;

const CURRENT: &str = "2026-07-28";

const TOOL: &str = "diff_package_versions";

/// The archives this suite is served from, instead of the registries.
const FIXTURES: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/fixtures/archives");

// ---------------------------------------------------------------------------
// What a client is told
// ---------------------------------------------------------------------------

/// The definition carries what an agent needs to call this correctly having
/// read nothing else, which is #23's question asked of the tool that exists.
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

    // The two tuning arguments have defaults, and the defaults are in the
    // schema rather than in prose: an agent that omits them should be able to
    // predict what it gets, because the values are part of the `diff_id` it
    // will be handed back.
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

    // The threshold's range, where an agent reads it rather than in the
    // sentence beside it. A value outside it is narrowed by the comparison
    // and not by the identifier, so the same diff asked for twice out of
    // range is two names for one answer — which the bound is what stops.
    let threshold = &tool["inputSchema"]["properties"]["similarity_threshold"];
    assert_eq!(threshold["minimum"], json!(0.0), "got {threshold}");
    assert_eq!(threshold["maximum"], json!(1.0), "got {threshold}");

    assert_eq!(
        tool["inputSchema"]["properties"]["registry"]["enum"],
        json!(["npm", "crates", "pypi"]),
        "the enum comes from `src/registry.rs` rather than from prose here, got {tool}"
    );

    assert_eq!(
        tool["outputSchema"]["type"], "object",
        "a tool answering with structured content declares its shape, got {tool}"
    );

    assert_eq!(
        tool["annotations"]["readOnlyHint"], true,
        "computing a diff changes nothing a caller can observe, and a client \
         deciding whether to ask for confirmation reads this, got {}",
        tool["annotations"]
    );
    assert_eq!(
        tool["annotations"]["idempotentHint"], true,
        "two published versions are immutable, so the same arguments give the \
         same diff, got {}",
        tool["annotations"]
    );
    assert_eq!(
        tool["annotations"]["openWorldHint"], true,
        "the arguments name a package on a registry, which is a world this \
         server does not control, got {}",
        tool["annotations"]
    );
}

/// The handle in the answer is described by the module that mints it.
///
/// This is the half a tool can take away without noticing: a doc comment on
/// the field overrides the description the type wrote, and what is lost is
/// the sentence saying a handle is passed back unchanged rather than built by
/// hand. The tools that take one declare the same type, so a description
/// written here would be this one answer disagreeing with all three of them —
/// and this is where an agent meets a handle first.
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

// ---------------------------------------------------------------------------
// What it answers
// ---------------------------------------------------------------------------

/// The tracer bullet: one version pair, summarised.
///
/// `diffable` 1.0.0 → 2.0.0 is built to have exactly one file of each status,
/// so every total below is a number worked out from the fixture rather than a
/// number this tool was asked for. The engine's counting rules are what make
/// them predictable: a removed file counts every line it had, an added file
/// every line it gained, a modified file the lines the comparison actually
/// touched, and a rename whose content did not change counts nothing.
///
/// - `README.md` is byte-identical: unchanged, 0 and 0.
/// - `src/index.js` changes one line of three: modified, 1 and 1.
/// - `src/removed.js` is one line and is gone: removed, 0 and 1.
/// - `src/added.js` is one line and is new: added, 1 and 0.
/// - `src/old-name.js` becomes `src/new-name.js` with the same two lines:
///   renamed, 0 and 0.
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

/// Both versions are named back, because a summary an agent files away should
/// say what it summarises without the call beside it.
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

/// The most-changed files come back in order, with the rename named.
///
/// The order is churn — lines added plus lines removed — and then path, so
/// that two files that moved the same amount do not swap places between two
/// calls. Churn is worked out from the fixture: `src/index.js` moves two
/// lines, `src/added.js` and `src/removed.js` one each, and the rename moves
/// none because its content is identical.
///
/// Unchanged files are not in the list at all. An agent asking what changed
/// should not have to filter the answer.
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

/// A rename is one file that moved, not a delete beside an add.
///
/// The totals say it and the listing says it, and both matter: a tool that
/// reported the pair as added-and-removed would have the same churn and tell
/// an agent that two files it can read are actually one.
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

/// The summary stays a summary, whatever the pair is.
///
/// `churny` modifies twenty-five files by one line each. Every one has the
/// same churn, so this is also the tie-break: without the sort by path the
/// twenty that survive the cut would be whichever twenty the tree walk
/// reached first.
///
/// The cut is what keeps this tool from needing a cursor. `get_diff_tree`
/// (#14) is the paginated one; this answer is bounded by construction.
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

// ---------------------------------------------------------------------------
// When it cannot answer
// ---------------------------------------------------------------------------

/// A version the registry does not have is something the model can act on.
///
/// So it is a result carrying `isError`, not a JSON-RPC error: the model
/// chose the version and is the one who can choose another. `tests/errors.rs`
/// holds the two channels apart in general; this is that rule reaching this
/// tool, on the argument only this tool has two of.
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

/// The same, on the other version, because a tool that fetches twice can get
/// this right in one direction and wrong in the other.
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

// ---------------------------------------------------------------------------
// The two tuning arguments
// ---------------------------------------------------------------------------

/// A reformatting is a change, until a caller says it is not.
///
/// `reformatted` 1.0.0 → 2.0.0 changes one line of three, and changes nothing
/// about it but the whitespace: a tab becomes four spaces and `x=1` becomes
/// `x = 1`. Both halves are asserted in one test on purpose — that the
/// argument reports zero changed files is only interesting beside the same
/// pair reporting one without it.
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

/// Lowering the threshold finds a rename the default does not.
///
/// `renamey` moves `src/handler.js` to `src/processor.js` and replaces five
/// of its fifteen lines. The engine scores that pair at ten unchanged lines
/// over twenty changes-and-unchanged — a similarity of 0.5 — which is under
/// the default 0.75 and over 0.4. So the same pair is a delete beside an add
/// at the default, and one moved file at 0.4.
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

// ---------------------------------------------------------------------------
// The handle, and what names a diff
// ---------------------------------------------------------------------------

/// The `diff_id` this tool hands back is the one `docs/cache-key.md` fixes.
///
/// The arguments below are the worked example in that document, and the
/// expected hash is read out of the golden vectors beside it — which were
/// generated from the document by a throwaway script rather than by either
/// implementation they check. So this is the tool held against the contract,
/// not against itself: #27 looks a result up by this string from TypeScript
/// that will never share a line of code with this crate.
#[tokio::test]
async fn the_diff_id_is_the_one_the_cache_key_document_fixes() {
    let vector = golden_vector("npm/zod");
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

/// Asking twice gives the same name for the same diff.
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

/// A→B is not B→A, in the name and in the answer.
///
/// Both halves matter. A different `diff_id` alone would be satisfied by a
/// tool that hashed the arguments and then ignored their order; the swapped
/// totals are what show the comparison itself was run the other way round.
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

    // What was added going forwards is what is removed coming back, and the
    // modified, renamed and unchanged counts are unmoved.
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

/// The handle reads back as the inputs it was minted from.
///
/// This is the property #14, #15 and #16 rest on: a reading tool that finds
/// nothing cached recovers the whole call from the handle and recomputes.
/// `DiffHandle::decode` refuses a handle whose halves disagree, so that it
/// decodes at all is most of the assertion; that it decodes to *these*
/// inputs is the rest.
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

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// The listed definition of `name`, or a panic naming what was listed.
async fn listed(name: &str) -> Value {
    let answer = post(json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "tools/list",
        "params": { "_meta": meta() },
    }))
    .await;

    let tools = answer["result"]["tools"]
        .as_array()
        .unwrap_or_else(|| panic!("tools/list should answer with an array, got {answer}"))
        .clone();

    tools
        .iter()
        .find(|tool| tool["name"] == name)
        .unwrap_or_else(|| {
            let names: Vec<&str> = tools.iter().filter_map(|t| t["name"].as_str()).collect();
            panic!("`{name}` should be listed, got {names:?}")
        })
        .clone()
}

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

/// One vector out of `fixtures/cache-key-vectors.json`, by name.
///
/// The file is the normative document's, generated from it rather than from
/// this crate, which is what makes a value read out of it an independent
/// expectation rather than a restatement of the code.
fn golden_vector(name: &str) -> Value {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/fixtures/cache-key-vectors.json"
    );
    let text = std::fs::read_to_string(path).expect("the golden vectors should read");
    let file: Value = serde_json::from_str(&text).expect("the golden vectors should parse");

    file["vectors"]
        .as_array()
        .expect("the file holds an array of vectors")
        .iter()
        .find(|vector| vector["name"] == name)
        .unwrap_or_else(|| panic!("`{name}` should be one of the golden vectors"))
        .clone()
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
