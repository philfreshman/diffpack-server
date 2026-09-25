//! `get_file_diff`, driven the way an agent drives it.
//!
//! One seam: the wire. `tools/list` for what a client is told and
//! `tools/call` for what it gets back, both through `router_with` over the
//! fixture archives. Testing here rather than at the handler is what keeps
//! the definition and the handler from drifting apart: a schema that stopped
//! describing what the handler reads is a confident wrong answer to a model,
//! and a handler test would go on passing through it.
//!
//! # Where an expected diff comes from
//!
//! The engine, wherever the engine can be asked. `get_diff_content` is
//! public, so the case where both versions have the file and they differ is
//! held against the engine's own output over the fixture's own bytes — which
//! is what makes the assertion able to disagree with this crate rather than
//! agree with it by construction.
//!
//! The other four cases are not asked of the engine. `build_patch` has been
//! public since `diffpack-engine` 0.4.0, but it is the function this tool
//! renders through, so an expectation read from it would agree with this
//! crate by construction. Those four are held against literals worked out by
//! hand from the table in #15, which is the contract they exist to reproduce.
//!
//! # What is deliberately not asserted here
//!
//! Where a cut falls, what the marker says, and that the ceiling is measured
//! on serialised bytes rather than on length. Those are `src/page.rs`'s and
//! `tests/page.rs` holds them against generated text. What this suite asserts
//! is that this tool goes *through* that module rather than around it.
//!
//! That the handle format is what it is. `tests/handle.rs` holds the four
//! ways a handle is refused against hand-built payloads. What is here is two
//! of them reaching *this* tool's argument, because a tool that took a handle
//! and verified it late would pass every test in that file.
//!
//! Nor that no `description` an agent reads names a Rust path — that is a
//! rule every tool is held to rather than a fact about this one, so
//! `tests/tools.rs` holds it over the whole of `tools/list`.

mod common;

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine as _;
use common::Client;
use diffpack_server::engine;
use diffpack_server::handle::{DiffHandle, Inputs};
use diffpack_server::page;
use diffpack_server::registry::Registry;
use serde_json::{json, Value};

const TOOL: &str = "get_file_diff";

// ---------------------------------------------------------------------------
// The bytes the fixtures hold
// ---------------------------------------------------------------------------
//
// Written out rather than read back out of the archive, so that an expected
// diff is computed from what `fixtures/archives/diffable-*.tgz` is known to
// contain rather than from whatever this crate's extractor returned. A test
// that took both sides from the extractor would agree with it about a file it
// had decoded wrongly.

/// `src/index.js` in `diffable` 1.0.0 — the file the second version edits.
const INDEX_FROM: &str = "export function greet(name) {\n  return \"Hello, \" + name;\n}\n";

/// The same file in 2.0.0.
const INDEX_TO: &str = "export function greet(name) {\n  return \"Hi, \" + name;\n}\n";

/// `src/handler.js` in `renamey` 1.0.0 — ten lines the second version keeps
/// and five it replaces, which is what makes the pair a rename and what
/// leaves enough unchanged context to trim.
const HANDLER: &str = "\
const shared00 = 0;
const shared01 = 0;
const shared02 = 0;
const shared03 = 0;
const shared04 = 0;
const shared05 = 0;
const shared06 = 0;
const shared07 = 0;
const shared08 = 0;
const shared09 = 0;
const alpha00 = 1;
const alpha01 = 1;
const alpha02 = 1;
const alpha03 = 1;
const alpha04 = 1;
";

/// The same file in 2.0.0, under the name it moved to.
const PROCESSOR: &str = "\
const shared00 = 0;
const shared01 = 0;
const shared02 = 0;
const shared03 = 0;
const shared04 = 0;
const shared05 = 0;
const shared06 = 0;
const shared07 = 0;
const shared08 = 0;
const shared09 = 0;
const bravo00 = 2;
const bravo01 = 2;
const bravo02 = 2;
const bravo03 = 2;
const bravo04 = 2;
";

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

    for field in ["handle", "path", "old_path", "context_lines", "max_bytes"] {
        assert!(
            tool["inputSchema"]["properties"][field].is_object(),
            "the input schema should describe `{field}`, got {}",
            tool["inputSchema"]
        );
    }

    assert_eq!(
        tool["inputSchema"]["required"],
        json!(["handle", "path"]),
        "a comparison and a file inside it are the two things a caller must \
         name; everything else has an answer without it: got {}",
        tool["inputSchema"]
    );
    for field in ["text", "truncated", "bytes", "isDiff"] {
        assert!(
            tool["outputSchema"]["properties"][field].is_object(),
            "the output schema should describe `{field}`, got {}",
            tool["outputSchema"]
        );
    }
    assert_eq!(
        tool["annotations"]["idempotentHint"],
        json!(true),
        "a handle names two published versions, which are immutable, so the \
         same patch is the same patch, got {}",
        tool["annotations"]
    );
}

/// The handle this tool takes is described by the module that mints it.
///
/// Four tools take one, and a sentence written here would be a fifth
/// description of the same string. What a caller has to know — where it comes
/// from, and that it is not built by hand — belongs to the one module that
/// knows it.
#[tokio::test]
async fn the_handle_this_tool_takes_is_described_by_the_module_that_mints_it() {
    let tool = listed(TOOL).await;
    let described = tool["inputSchema"]["properties"]["handle"]["description"]
        .as_str()
        .unwrap_or_else(|| panic!("the handle carries a description, got {tool}"));

    assert!(
        described.contains("diff_package_versions"),
        "a caller reading this should be told which call produces one: got \
         {described}"
    );
    assert!(
        described.contains("passed back"),
        "and that it is passed back rather than written: got {described}"
    );
}

/// The context argument documents the number that binds.
///
/// The default reaches a model from the schema rather than from a sentence
/// this tool wrote, so the number an agent reads and the number the handler
/// applies cannot be two different threes.
#[tokio::test]
async fn the_context_argument_documents_the_default_that_binds() {
    let tool = listed(TOOL).await;
    let context = tool["inputSchema"]["properties"]["context_lines"].clone();

    assert_eq!(
        context["default"],
        json!(3),
        "the default in the schema is the one the handler uses when the \
         argument is absent, which `context_defaults_to_three_lines` holds \
         from the other end: got {context}"
    );

    let described = context["description"]
        .as_str()
        .unwrap_or_else(|| panic!("the argument carries a description, got {context}"));
    assert!(
        described.contains("full"),
        "an agent that wants the whole file has to be told the word for it, \
         since nothing about a number suggests one: got {described}"
    );
}

/// The description says the three things an agent would otherwise get wrong.
///
/// Each is a case where a correct answer reads as a wrong one to a caller
/// that was not told: an answer that is a file rather than a patch, a renamed
/// file that reads as added unless its old path comes with it, and a patch
/// that is short because context was trimmed rather than because little
/// changed.
#[tokio::test]
async fn the_description_says_what_an_agent_would_otherwise_get_wrong() {
    let tool = listed(TOOL).await;
    let description = tool["description"]
        .as_str()
        .unwrap_or_else(|| panic!("a tool carries a description, got {tool}"));

    for said in ["isDiff", "old_path", "context_lines"] {
        assert!(
            description.contains(said),
            "the description should name `{said}`, which is a case where a \
             correct answer reads as a wrong one to a caller that was not \
             told: got {description}"
        );
    }
}

// ---------------------------------------------------------------------------
// The five cases
// ---------------------------------------------------------------------------

/// A file both versions have and that changed is the engine's own diff.
///
/// The one case the engine can be asked about directly: `get_diff_content` is
/// public, so the expectation is its output over the fixture's bytes rather
/// than a string written beside the implementation. `context_lines: "full"`
/// because that is the setting #15 defines as the engine's output untouched;
/// what a trimmed answer looks like is a question further down this file.
#[tokio::test]
async fn a_changed_file_is_the_engines_own_diff() {
    let answer = patch(json!({
        "handle": diffable(),
        "path": "src/index.js",
        "context_lines": "full",
    }))
    .await;

    assert_eq!(
        answer["text"],
        json!(engine::get_diff_content(
            "src/index.js",
            INDEX_FROM,
            INDEX_TO,
            false
        )),
        "a file both versions have and that changed is rendered by the engine \
         rather than by this crate, byte for byte: got {answer}"
    );
    assert_eq!(
        answer["isDiff"],
        json!(true),
        "a file that changed is a diff, which is what tells a viewer to render \
         it as one: got {answer}"
    );
}

/// A file only the second version has is every line of it, added.
///
/// A literal rather than the engine, for the reason in the header: the
/// renderer that produces this is the one this tool calls, so asking it would
/// prove nothing. What is written out is the table in #15 — `/dev/null` on
/// the left, every line prefixed — down to the trailing `+ ` that the
/// engine's split leaves on a file ending in a newline.
#[tokio::test]
async fn a_file_only_the_second_version_has_is_every_line_added() {
    let answer = patch(json!({
        "handle": diffable(),
        "path": "src/added.js",
        "context_lines": "full",
    }))
    .await;

    assert_eq!(
        answer["text"],
        json!("--- /dev/null\n+++ to/src/added.js\n+ export const fresh = true;\n+ "),
        "a file the first version does not have is `/dev/null` against the \
         second version's, every line prefixed: got {answer}"
    );
    assert_eq!(
        answer["isDiff"],
        json!(true),
        "an added file is a diff — there is a `+` on every line of it: got {answer}"
    );
}

/// A file only the first version has is every line of it, removed.
///
/// The mirror of the case above, and the `path` argument reads the other way
/// round with it: there is nothing at this path in the second version, so
/// what was passed is where the file *was*.
#[tokio::test]
async fn a_file_only_the_first_version_has_is_every_line_removed() {
    let answer = patch(json!({
        "handle": diffable(),
        "path": "src/removed.js",
        "context_lines": "full",
    }))
    .await;

    assert_eq!(
        answer["text"],
        json!("--- from/src/removed.js\n+++ /dev/null\n- export const gone = true;\n- "),
        "a file the second version does not have is the first version's \
         against `/dev/null`, every line prefixed: got {answer}"
    );
    assert_eq!(
        answer["isDiff"],
        json!(true),
        "a removed file is a diff — there is a `-` on every line of it: got {answer}"
    );
}

/// A file both versions have byte for byte is the file, not a diff of it.
///
/// The case `isDiff` exists for. Rendering it as a patch of nothing but
/// context lines would be true and useless — an agent would parse a file as a
/// diff, strip a prefix off every line and read a file it had subtly altered.
/// So the file comes back as itself and the flag says to read it as one.
#[tokio::test]
async fn a_file_neither_version_touched_is_the_file_itself() {
    let answer = patch(json!({
        "handle": diffable(),
        "path": "README.md",
        "context_lines": "full",
    }))
    .await;

    assert_eq!(
        answer["text"],
        json!("# diffable\n"),
        "a file that did not change is its own content, with no header and no \
         prefix on any line: got {answer}"
    );
    assert_eq!(
        answer["isDiff"],
        json!(false),
        "there is no diff to read here, which is what tells a caller to render \
         this as a file: got {answer}"
    );
}

/// A path neither version has is a sentence saying so, not a failure.
///
/// The engine's fifth case, and it is an answer rather than an error on
/// purpose: a comparison is a pair of versions and "this path is in neither
/// of them" is a fact about the pair. What a caller does next is look at the
/// tree, which is what the sentence is for.
#[tokio::test]
async fn a_path_in_neither_version_says_so_rather_than_failing() {
    let answer = patch(json!({
        "handle": diffable(),
        "path": "src/never-was.js",
        "context_lines": "full",
    }))
    .await;

    assert_eq!(
        answer["text"],
        json!("File not present in either version."),
        "a path in neither version is the engine's own sentence, word for \
         word: got {answer}"
    );
    assert_eq!(
        answer["isDiff"],
        json!(false),
        "a sentence is not a patch, and a caller that parsed it as one would \
         read a hunk out of prose: got {answer}"
    );
}

// ---------------------------------------------------------------------------
// A file that moved
// ---------------------------------------------------------------------------

/// A renamed file is its old path in the first version against its new one in
/// the second.
///
/// The header names the new path on both sides, which is the engine's and is
/// worth reading twice: the `--- from/` line says `src/processor.js` even
/// though those lines came out of `src/handler.js`. It is what the browser
/// shows, so it is what an agent is shown.
#[tokio::test]
async fn a_renamed_file_diffs_where_it_was_against_where_it_is() {
    let answer = patch(json!({
        "handle": handle("renamey", "1.0.0", "2.0.0", false),
        "path": "src/processor.js",
        "old_path": "src/handler.js",
        "context_lines": "full",
    }))
    .await;

    assert_eq!(
        answer["text"],
        json!(engine::get_diff_content(
            "src/processor.js",
            HANDLER,
            PROCESSOR,
            false
        )),
        "a rename diffs the two files rather than reporting one added and one \
         removed: got {answer}"
    );
}

/// Omitting `old_path` on a renamed file reads it as added, not as a rename.
///
/// The argument is doing the work rather than a rename being detected here a
/// second time: without it there is no file at this path in the first
/// version, and "no file there" is the added case. Asserting it is what stops
/// the test above from passing on a tool that ignored the argument and
/// guessed.
#[tokio::test]
async fn a_rename_without_its_old_path_reads_as_an_added_file() {
    let answer = patch(json!({
        "handle": handle("renamey", "1.0.0", "2.0.0", false),
        "path": "src/processor.js",
        "context_lines": "full",
    }))
    .await;

    assert!(
        answer["text"]
            .as_str()
            .is_some_and(|text| text.starts_with("--- /dev/null")),
        "with nothing said about where the file was, the first version has no \
         file at this path: got {answer}"
    );
}

// ---------------------------------------------------------------------------
// What the handle fixes and this tool does not take
// ---------------------------------------------------------------------------

/// `src/main.js` in `reformatted` 1.0.0 — a tab and no spaces around `=`.
const MAIN_FROM: &str = "function main() {\n\tlet x=1;\n}\n";

/// The same file in 2.0.0, reindented and spaced out. Nothing else moved.
const MAIN_TO: &str = "function main() {\n    let x = 1;\n}\n";

/// A handle minted with `ignore_whitespace` carries it into the patch.
///
/// The two calls below differ in the handle and in nothing else, and they
/// come back different — which is the whole of what "fixed by the `diff_id`"
/// means. A reformatting is the only change in this pair, so ignoring
/// whitespace leaves a diff with no `+` and no `-` line in it at all.
#[tokio::test]
async fn whitespace_is_ignored_when_the_handle_was_minted_that_way() {
    let exactly = patch(json!({
        "handle": handle("reformatted", "1.0.0", "2.0.0", false),
        "path": "src/main.js",
        "context_lines": "full",
    }))
    .await;

    let ignoring = patch(json!({
        "handle": handle("reformatted", "1.0.0", "2.0.0", true),
        "path": "src/main.js",
        "context_lines": "full",
    }))
    .await;

    assert_eq!(
        exactly["text"],
        json!(engine::get_diff_content(
            "src/main.js",
            MAIN_FROM,
            MAIN_TO,
            false
        )),
        "a handle minted exactly renders every reindented line as a change: \
         got {exactly}"
    );
    assert_eq!(
        ignoring["text"],
        json!(engine::get_diff_content(
            "src/main.js",
            MAIN_FROM,
            MAIN_TO,
            true
        )),
        "a handle minted ignoring whitespace renders the same pair with the \
         reindentation disregarded: got {ignoring}"
    );
    assert_ne!(
        exactly["text"], ignoring["text"],
        "if the two read alike then the setting reached nothing, and this pair \
         differs in nothing but whitespace"
    );
}

/// Whitespace is not something a caller may change after the fact.
///
/// Two option sets are two diffs, so a tool that took this argument would be
/// answering about a comparison nobody computed — and the `diff_id` beside it
/// would name the other one. The refusal is the protocol channel, because a
/// model cannot fix it by rewording: the way to a different setting is a new
/// call to the tool that mints handles.
#[tokio::test]
async fn whitespace_is_not_an_argument_this_tool_accepts() {
    let answer = Client::fixture()
        .post(json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "tools/call",
            "params": {
                "name": TOOL,
                "arguments": {
                    "handle": diffable(),
                    "path": "src/index.js",
                    "ignore_whitespace": true,
                },
            },
        }))
        .await;

    assert_eq!(
        answer["error"]["code"],
        json!(-32602),
        "an argument fixed by the handle is refused rather than quietly \
         ignored, which is the difference between being told and being \
         answered about another diff: got {answer}"
    );
}

/// Nor is the threshold that decided what a rename is.
#[tokio::test]
async fn the_similarity_threshold_is_not_an_argument_this_tool_accepts() {
    let answer = Client::fixture()
        .post(json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "tools/call",
            "params": {
                "name": TOOL,
                "arguments": {
                    "handle": diffable(),
                    "path": "src/index.js",
                    "similarity_threshold": 0.4,
                },
            },
        }))
        .await;

    assert_eq!(
        answer["error"]["code"],
        json!(-32602),
        "the threshold is fixed by the handle for the reason whitespace is: \
         got {answer}"
    );
}

// ---------------------------------------------------------------------------
// How much of the file comes with the change
// ---------------------------------------------------------------------------

/// Three lines of context is a hunk with the header a unified diff has.
///
/// The expectation is written out rather than derived. `renamey` keeps ten
/// lines and replaces five, so both sides of the hunk start at line 8 — three
/// lines of context before the first change — and run for eight: the three
/// context lines plus five of the caller's own side. Seven unchanged lines
/// above that are what trimming is for.
#[tokio::test]
async fn three_lines_of_context_is_a_hunk_with_line_numbers() {
    let answer = patch(json!({
        "handle": handle("renamey", "1.0.0", "2.0.0", false),
        "path": "src/processor.js",
        "old_path": "src/handler.js",
        "context_lines": 3,
    }))
    .await;

    assert_eq!(
        answer["text"],
        json!(
            "\
--- from/src/processor.js
+++ to/src/processor.js
@@ -8,8 +8,8 @@
  const shared07 = 0;
  const shared08 = 0;
  const shared09 = 0;
- const alpha00 = 1;
- const alpha01 = 1;
- const alpha02 = 1;
- const alpha03 = 1;
- const alpha04 = 1;
+ const bravo00 = 2;
+ const bravo01 = 2;
+ const bravo02 = 2;
+ const bravo03 = 2;
+ const bravo04 = 2;"
        ),
        "the answer keeps three unchanged lines before the change, drops the \
         seven above them, and says where what is left sits in each file: got \
         {answer}"
    );
}

/// Omitting the argument is asking for three.
#[tokio::test]
async fn context_defaults_to_three_lines() {
    let asked = patch(json!({
        "handle": handle("renamey", "1.0.0", "2.0.0", false),
        "path": "src/processor.js",
        "old_path": "src/handler.js",
        "context_lines": 3,
    }))
    .await;

    let silent = patch(json!({
        "handle": handle("renamey", "1.0.0", "2.0.0", false),
        "path": "src/processor.js",
        "old_path": "src/handler.js",
    }))
    .await;

    assert_eq!(
        silent["text"], asked["text"],
        "a caller that said nothing about context is answered the way one that \
         asked for three is, rather than being handed the whole file: got \
         {silent}"
    );
}

/// A hunk one line long on a side writes that side's start and no count.
///
/// The unified-diff rule, and the one a parser written against `git diff`
/// will be holding us to. `churny` changes one line of a one-line file, so
/// both sides are the single-line case at once.
#[tokio::test]
async fn a_single_line_side_is_written_without_a_count() {
    let answer = patch(json!({
        "handle": handle("churny", "1.0.0", "2.0.0", false),
        "path": "src/file00.js",
        "context_lines": 3,
    }))
    .await;

    assert_eq!(
        answer["text"],
        json!(
            "\
--- from/src/file00.js
+++ to/src/file00.js
@@ -1 +1 @@
- export const value = 0;
+ export const value = 100;"
        ),
        "a side that is one line long is its start line and nothing else: got \
         {answer}"
    );
}

/// `\"full\"` is the engine's output with nothing done to it.
///
/// The other half of the criterion the hunks above are the first half of: at
/// this setting there is no header to add and no line to drop, so what comes
/// back is what the engine rendered.
#[tokio::test]
async fn full_context_is_the_engines_output_untouched() {
    let answer = patch(json!({
        "handle": handle("renamey", "1.0.0", "2.0.0", false),
        "path": "src/processor.js",
        "old_path": "src/handler.js",
        "context_lines": "full",
    }))
    .await;

    assert_eq!(
        answer["text"],
        json!(engine::get_diff_content(
            "src/processor.js",
            HANDLER,
            PROCESSOR,
            false
        )),
        "at `full` the engine's output is passed through, `@@` header and all \
         — which is to say without one: got {answer}"
    );
}

/// A file only one version has gets the empty side `git` writes for it.
///
/// There is nothing to trim — every line is a change — so what a numeric
/// `context_lines` adds here is the line numbers, and the side that has no
/// lines is `0,0` rather than being left out.
#[tokio::test]
async fn a_one_sided_file_names_the_side_that_has_no_lines() {
    let added = patch(json!({
        "handle": diffable(),
        "path": "src/added.js",
        "context_lines": 3,
    }))
    .await;

    let removed = patch(json!({
        "handle": diffable(),
        "path": "src/removed.js",
        "context_lines": 3,
    }))
    .await;

    assert_eq!(
        added["text"],
        json!(
            "--- /dev/null\n+++ to/src/added.js\n@@ -0,0 +1,2 @@\n+ export const fresh = true;\n+ "
        ),
        "a file the first version does not have has nothing on the left of the \
         header: got {added}"
    );
    assert_eq!(
        removed["text"],
        json!(
            "--- from/src/removed.js\n+++ /dev/null\n@@ -1,2 +0,0 @@\n\
             - export const gone = true;\n- "
        ),
        "a file the second version does not have has nothing on the right of \
         it: got {removed}"
    );
}

/// No context at all is the changed lines and the header, and nothing else.
#[tokio::test]
async fn no_context_is_the_changed_lines_alone() {
    let answer = patch(json!({
        "handle": handle("renamey", "1.0.0", "2.0.0", false),
        "path": "src/processor.js",
        "old_path": "src/handler.js",
        "context_lines": 0,
    }))
    .await;

    assert_eq!(
        answer["text"],
        json!(
            "\
--- from/src/processor.js
+++ to/src/processor.js
@@ -11,5 +11,5 @@
- const alpha00 = 1;
- const alpha01 = 1;
- const alpha02 = 1;
- const alpha03 = 1;
- const alpha04 = 1;
+ const bravo00 = 2;
+ const bravo01 = 2;
+ const bravo02 = 2;
+ const bravo03 = 2;
+ const bravo04 = 2;"
        ),
        "with no context asked for, the five replaced lines start at line \
         eleven of each file and nothing around them comes with them: got \
         {answer}"
    );
}

/// Trimming drops the lines below the last change as well as those above the
/// first.
///
/// Every other hunk in this file runs to the end of the body it came from —
/// `renamey` replaces its last five lines and `churny` its only one — so a
/// trimmer that cut the top of a file and kept the bottom would pass all of
/// them. `src/index.js` changes the middle line of three, which is the shape
/// that tells the two apart: the `}` below the change goes with the
/// declaration above it.
#[tokio::test]
async fn a_change_in_the_middle_drops_the_lines_below_it_too() {
    let answer = patch(json!({
        "handle": diffable(),
        "path": "src/index.js",
        "context_lines": 0,
    }))
    .await;

    assert_eq!(
        answer["text"],
        json!(
            "\
--- from/src/index.js
+++ to/src/index.js
@@ -2 +2 @@
-   return \"Hello, \" + name;
+   return \"Hi, \" + name;"
        ),
        "with no context asked for, the one line that changed is all that is \
         left of a three-line file, and the hunk starts where the change is \
         rather than where the file does: got {answer}"
    );
}

/// A diff with no changed line in it trims to its header.
///
/// The case a whitespace-only change makes when the handle says to ignore
/// whitespace: the two files differ in bytes, so this is a diff, and no line
/// differs, so there is no hunk. A header on its own is what a unified diff
/// of nothing looks like, and it is the honest answer — the full setting is
/// there for a caller that wants to see the file anyway.
#[tokio::test]
async fn a_diff_with_nothing_changed_in_it_is_its_header() {
    let answer = patch(json!({
        "handle": handle("reformatted", "1.0.0", "2.0.0", true),
        "path": "src/main.js",
        "context_lines": 3,
    }))
    .await;

    assert_eq!(
        answer["text"],
        json!("--- from/src/main.js\n+++ to/src/main.js"),
        "no line changed under this handle's setting, so there is no hunk to \
         show: got {answer}"
    );
    assert_eq!(
        answer["isDiff"],
        json!(true),
        "the two files are not the same bytes, which is what makes this a diff \
         rather than a file: got {answer}"
    );
}

/// Every line a trimmed answer carries, the full answer carries too.
///
/// What "presentation over the engine's output" means, held as a property
/// rather than read off one example: trimming drops lines and adds hunk
/// headers, and it invents no content. If it ever rewrote a line — re-wrapped
/// it, changed a prefix, normalised whitespace — the line would not be found
/// in the full answer and this fails.
///
/// Asserted over every file of two comparisons, so it is a claim about the
/// trimmer rather than about the one file that was convenient.
#[tokio::test]
async fn a_trimmed_answer_carries_no_line_the_full_answer_does_not() {
    for (package, path) in [
        ("renamey", "src/processor.js"),
        ("churny", "src/file00.js"),
        ("diffable", "src/index.js"),
        ("diffable", "src/added.js"),
        ("diffable", "src/removed.js"),
        ("reformatted", "src/main.js"),
    ] {
        let mut arguments = json!({
            "handle": handle(package, "1.0.0", "2.0.0", false),
            "path": path,
        });
        if package == "renamey" {
            arguments["old_path"] = json!("src/handler.js");
        }

        let mut full = arguments.clone();
        full["context_lines"] = json!("full");

        let trimmed = patch(arguments).await;
        let full = patch(full).await;

        let whole: Vec<&str> = text(&full).lines().collect();
        let kept: Vec<&str> = text(&trimmed)
            .lines()
            .filter(|line| !line.starts_with("@@ "))
            .collect();

        let mut remaining = whole.iter();
        for line in &kept {
            assert!(
                remaining.any(|whole| whole == line),
                "`{line}` is in the trimmed answer for `{package}` `{path}` but \
                 not in what is left of the full one, so trimming did more than \
                 drop lines\n--- full ---\n{}\n--- trimmed ---\n{}",
                text(&full),
                text(&trimmed),
            );
        }

        assert!(
            kept.len() <= whole.len(),
            "trimming `{package}` `{path}` produced more lines than it was \
             given"
        );
    }
}

// ---------------------------------------------------------------------------
// How much of it comes back
// ---------------------------------------------------------------------------

/// A patch over `max_bytes` is cut, says so, and states its real size.
///
/// The cut itself is `src/page.rs`'s and `tests/page.rs` holds where it
/// falls. What is here is that this tool goes through that module rather than
/// handing back whatever it rendered.
#[tokio::test]
async fn a_patch_over_max_bytes_is_cut_and_says_so() {
    let whole = patch(json!({
        "handle": diffable(),
        "path": "src/index.js",
        "context_lines": "full",
    }))
    .await;

    let cut = patch(json!({
        "handle": diffable(),
        "path": "src/index.js",
        "context_lines": "full",
        "max_bytes": 20,
    }))
    .await;

    assert_eq!(
        cut["truncated"],
        json!(true),
        "a patch cut short says so, because a silently shortened diff is how \
         an agent concludes a change was not made: got {cut}"
    );
    assert!(
        text(&cut).contains("truncated by diffpack"),
        "the text itself carries the marker, for a reader that never looks at \
         the flag beside it: got {cut}"
    );
    assert_eq!(
        cut["bytes"], whole["bytes"],
        "the size reported is the whole patch's and not the piece that came \
         back, or an agent is told a diff it has seen a fifth of is a fifth \
         long: got {cut}"
    );
}

/// The size a cut is measured against is the patch at the context asked for.
///
/// Trimming happens before the cut, which is what makes `context_lines` worth
/// having: a caller that asked for three lines of context is not charged the
/// response budget for the lines it asked to be spared.
#[tokio::test]
async fn the_reported_size_is_the_patch_at_the_context_asked_for() {
    let trimmed = patch(json!({
        "handle": handle("renamey", "1.0.0", "2.0.0", false),
        "path": "src/processor.js",
        "old_path": "src/handler.js",
        "context_lines": 3,
    }))
    .await;

    let full = patch(json!({
        "handle": handle("renamey", "1.0.0", "2.0.0", false),
        "path": "src/processor.js",
        "old_path": "src/handler.js",
        "context_lines": "full",
    }))
    .await;

    let (trimmed, full) = (
        trimmed["bytes"].as_u64().expect("a size is a number"),
        full["bytes"].as_u64().expect("a size is a number"),
    );

    assert!(
        trimmed < full,
        "the trimmed patch should be the smaller of the two, since seven lines \
         of this file were dropped: got {trimmed} against {full}"
    );
}

/// A file larger than a response is cut with nothing asked for.
///
/// `odd-files` ships a two-megabyte file and has one version, so comparing it
/// against itself is the largest answer this suite can produce — a file
/// neither version touched, carried whole. The server's own cap is what stops
/// it, because a response over the platform's limit is not a long answer but
/// an error with nothing in it.
///
/// What this does not prove is the same cap over a *diff* that large, which
/// no pair of fixture archives can build. It is the same call to the same
/// module either way, and `tests/page.rs` is what holds where the cut falls.
#[tokio::test]
async fn an_answer_larger_than_a_response_is_cut_without_being_asked() {
    let sent = Client::fixture()
        .respond(json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "tools/call",
            "params": {
                "name": TOOL,
                "arguments": {
                    "handle": handle("odd-files", "1.0.0", "1.0.0", false),
                    "path": "big.txt",
                },
            },
        }))
        .await;

    let frame = sent.frame();
    let answer = sent.result()["structuredContent"].clone();

    assert_eq!(
        answer["truncated"],
        json!(true),
        "a two-megabyte answer does not fit under the response ceiling and is \
         cut whether or not a cap was asked for: got {}",
        answer["bytes"]
    );
    assert_eq!(
        answer["bytes"],
        json!(2_000_000),
        "the size reported is the whole file's: got {}",
        answer["bytes"]
    );
    assert!(
        frame.len() < page::RESPONSE_CEILING,
        "the frame this server wrote is {} bytes, over the {} the platform \
         will carry",
        frame.len(),
        page::RESPONSE_CEILING,
    );
}

// ---------------------------------------------------------------------------
// What is refused
// ---------------------------------------------------------------------------

/// A handle that is not one never reaches the handler.
///
/// `tests/handle.rs` holds the four ways a handle is refused. What is asserted
/// here is that this tool's argument is that type rather than a string it
/// checks itself: a tool that verified late would answer `-32603` or, worse,
/// fetch something first.
#[tokio::test]
async fn a_handle_that_is_not_one_is_a_protocol_error() {
    let answer = Client::fixture()
        .post(json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "tools/call",
            "params": {
                "name": TOOL,
                "arguments": { "handle": "not-a-handle", "path": "src/index.js" },
            },
        }))
        .await;

    assert_eq!(
        answer["error"]["code"],
        json!(-32602),
        "a handle a client made up is the client's mistake and not something a \
         model can reword its way out of: got {answer}"
    );
}

/// A handle whose two halves disagree is refused before anything is fetched.
#[tokio::test]
async fn a_handle_whose_halves_disagree_is_a_protocol_error() {
    let answer = Client::fixture()
        .post(json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "tools/call",
            "params": {
                "name": TOOL,
                "arguments": { "handle": edited(), "path": "src/index.js" },
            },
        }))
        .await;

    assert_eq!(
        answer["error"]["code"],
        json!(-32602),
        "a handle whose `diff_id` does not name the inputs beside it names one \
         diff and describes another: got {answer}"
    );
}

/// A version the registry does not have is a tool error naming it.
///
/// The model channel rather than the protocol one: the handle decoded and the
/// arguments were well formed, and what went wrong is something an agent can
/// act on by asking for a version that exists.
#[tokio::test]
async fn a_version_the_registry_does_not_have_names_what_was_not_found() {
    let result = call(json!({
        "handle": handle("diffable", "1.0.0", "9.9.9", false),
        "path": "src/index.js",
    }))
    .await;

    assert_eq!(
        result["isError"],
        json!(true),
        "a version that is not published cannot be diffed: got {result}"
    );

    let message = result["content"][0]["text"]
        .as_str()
        .expect("a tool error carries text for the model");
    assert!(
        message.contains("diffable") && message.contains("9.9.9"),
        "the message should name the package and the version that is missing, \
         since a bare `not found` leaves an agent guessing which of the two \
         was wrong: got {message}"
    );
}

/// A package the registry does not have is a tool error naming it.
///
/// A crate rather than an npm package, because the fixture set's way of
/// saying "the registry serves nothing here" is a `null` entry and that is
/// where it has one.
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

    let result = call(json!({ "handle": handle, "path": "src/lib.rs" })).await;

    assert_eq!(
        result["isError"],
        json!(true),
        "a package that does not exist cannot be diffed: got {result}"
    );
    assert!(
        result["content"][0]["text"]
            .as_str()
            .is_some_and(|message| message.contains("not-a-real-crate")),
        "the message should name what was asked for: got {result}"
    );
}

/// A directory is refused rather than reported as a path in neither version.
///
/// The one place this tool departs from the engine on purpose. A directory
/// has no content, so the engine's reading of it is "absent on both sides",
/// and reproducing that answers `src` — a directory both versions ship — with
/// a sentence saying it is in neither of them. That sentence is wrong and an
/// agent has nothing to doubt it with. The remedy is a different path, which
/// is a thing to be told.
#[tokio::test]
async fn a_directory_is_refused_rather_than_called_absent() {
    let result = call(json!({
        "handle": handle("moved", "1.0.0", "2.0.0", false),
        "path": "src",
    }))
    .await;

    assert_eq!(
        result["isError"],
        json!(true),
        "`src` is a directory of this package's, and saying it is in neither \
         version would be false about a path both versions have: got {result}"
    );

    let message = result["content"][0]["text"]
        .as_str()
        .expect("a tool error carries text for the model");
    assert!(
        message.contains("src") && message.contains("directory"),
        "the message should name the path and say what it is, since the remedy \
         is to ask for a file inside it: got {message}"
    );
}

/// So is a directory only one version has.
///
/// `moved` renames `src/legacy/reporter.js` to `src/reporter.js`, so
/// `src/legacy` is a directory in 1.0.0 and nothing in 2.0.0. There is no
/// file at that path in either version, so there is nothing to diff, and the
/// refusal names the version that has the directory. Both ways round: one way
/// the tree has the directory, the other way a rename emptied it and only the
/// file maps have it.
#[tokio::test]
async fn a_directory_only_one_version_has_is_refused() {
    for (from, to) in [("1.0.0", "2.0.0"), ("2.0.0", "1.0.0")] {
        let result = call(json!({
            "handle": handle("moved", from, to, false),
            "path": "src/legacy",
        }))
        .await;

        assert_eq!(
            result["isError"],
            json!(true),
            "{from} → {to}: `src/legacy` is a directory in 1.0.0 and missing \
             from 2.0.0, so there is no file to diff: got {result}"
        );

        let message = result["content"][0]["text"]
            .as_str()
            .expect("a tool error carries text for the model");
        assert!(
            message.contains("`src/legacy`")
                && message.contains("1.0.0")
                && message.contains("directory"),
            "{from} → {to}: the refusal names the path and the version it is a \
             directory in: got {message}"
        );
    }
}

/// So is the root, which is the directory everything else is in.
///
/// The tree names its root `/`, so asking for `/` is asking for a directory
/// and gets that refusal. Until #93 it was told `/` was in neither version,
/// because the refusal was made out of the file maps and those have no entry
/// for the root. #97 is where the other tools' reading of `/` is settled.
#[tokio::test]
async fn the_root_is_refused_as_a_directory() {
    let result = call(json!({ "handle": diffable(), "path": "/" })).await;

    assert_eq!(
        result["isError"],
        json!(true),
        "`/` is the root of the comparison, a directory both versions have: \
         got {result}"
    );

    let message = result["content"][0]["text"]
        .as_str()
        .expect("a tool error carries text for the model");
    assert!(
        message.contains("directory"),
        "the root gets the refusal every directory gets: got {message}"
    );
}

// ---------------------------------------------------------------------------
// Getting there
// ---------------------------------------------------------------------------

/// A handle for `diffable` 1.0.0 → 2.0.0, minted rather than fetched.
///
/// The tool that mints one is called where the *agreement* between the two is
/// what is being asserted. Everywhere else a handle is just the argument, and
/// minting it here is one call rather than two — the shape
/// `tests/get_diff_tree.rs` settled on.
fn diffable() -> String {
    handle("diffable", "1.0.0", "2.0.0", false)
}

/// A handle for one npm comparison, at the defaults `diff_package_versions`
/// would have used unless `ignore_whitespace` says otherwise.
fn handle(package: &str, from: &str, to: &str, ignore_whitespace: bool) -> String {
    DiffHandle::mint(Inputs {
        registry: Registry::Npm,
        package: package.to_owned(),
        from_version: from.to_owned(),
        to_version: to.to_owned(),
        similarity_threshold: 0.75,
        ignore_whitespace,
    })
    .encode()
}

/// The answer this tool gives to `arguments`, or a panic naming the failure.
///
/// The structured half, which is where a patch's `text`, `isDiff` and the
/// three fields describing a cut arrive.
async fn patch(arguments: Value) -> Value {
    let result = call(arguments).await;

    assert_ne!(
        result["isError"],
        json!(true),
        "expected a patch, got a tool error: {result}"
    );

    result["structuredContent"].clone()
}

/// A handle with its package changed and its `diff_id` left alone.
///
/// What an agent does to a handle it can read part of: change the thing it is
/// asking about and leave the identifier, which looks like an internal
/// detail. The encoding is spelled out here rather than taken from the crate,
/// so this is a forgery a client could send rather than one this server
/// helped build.
fn edited() -> String {
    let encoded = diffable();
    let encoded = encoded
        .strip_prefix("d1:")
        .expect("a handle this server minted");
    let payload = URL_SAFE_NO_PAD
        .decode(encoded)
        .expect("a handle's payload is base64url");
    let mut payload: Value = serde_json::from_slice(&payload).expect("a handle's payload is JSON");

    payload["package"] = json!("something-else");

    format!("d1:{}", URL_SAFE_NO_PAD.encode(payload.to_string()))
}

/// The `text` of an answer, or a panic naming what came back instead.
fn text(answer: &Value) -> &str {
    answer["text"]
        .as_str()
        .unwrap_or_else(|| panic!("a patch carries text, got {answer}"))
}

/// The listed definition of `name`, or a panic naming what was listed.
async fn listed(name: &str) -> Value {
    Client::fixture().listed(name).await
}

/// Call this tool with `arguments`, returning the `result`.
async fn call(arguments: Value) -> Value {
    Client::fixture().call(TOOL, arguments).await
}
