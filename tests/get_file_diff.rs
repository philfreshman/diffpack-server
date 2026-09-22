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
//! The other four cases cannot be asked for. `build_diff_result` is a private
//! `fn` in `diffpack-engine` 0.3.0 and `get_diff_for_path` is
//! `#[wasm_bindgen]`, so neither is callable from here and `src/engine.rs`
//! re-exports neither. Those four are held against literals worked out by
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

use axum::body::Body;
use axum::http::Request;
use diffpack_server::engine;
use diffpack_server::handle::{DiffHandle, Inputs};
use diffpack_server::mcp::Diffpack;
use diffpack_server::registry::Registry;
use diffpack_server::router;
use diffpack_server::tools::Ctx;
use http_body_util::BodyExt;
use serde_json::{json, Value};
use tower::ServiceExt;

const CURRENT: &str = "2026-07-28";

const TOOL: &str = "get_file_diff";

/// The fixture sets this suite is served from, instead of the registries.
///
/// The root rather than one seam's directory inside it: `Ctx::fixture` gives
/// every seam a fixture adapter, so nothing this suite builds can reach a
/// registry — including a seam this tool does not use today.
const FIXTURES: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/fixtures");

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
/// renderer that produces this is private to `diffpack-engine` and there is
/// nothing to call. What is written out is the table in #15 — `/dev/null` on
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
    let answer = post(json!({
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
            "_meta": meta(),
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
    let answer = post(json!({
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
            "_meta": meta(),
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
        json!("--- from/src/removed.js\n+++ /dev/null\n@@ -1,2 +0,0 @@\n- export const gone = true;\n- "),
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
// What is refused
// ---------------------------------------------------------------------------

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

/// The `text` of an answer, or a panic naming what came back instead.
fn text(answer: &Value) -> &str {
    answer["text"]
        .as_str()
        .unwrap_or_else(|| panic!("a patch carries text, got {answer}"))
}

/// Call this tool with `arguments`, returning the `result` — or panicking with
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
    let (_, answer) = respond(body).await;
    answer
}

/// The same request, with the body the client received beside the answer.
///
/// The bytes rather than the structure, because that is what the response
/// ceiling bounds: what Vercel refuses is the frame this server wrote, and
/// two frames that parse alike can still differ in size.
async fn respond(body: Value) -> (String, Value) {
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
        || Ok(Diffpack::with_ctx(Ctx::fixture(FIXTURES))),
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

    let answer = serde_json::from_slice(&bytes).unwrap_or_else(|e| {
        panic!(
            "expected a JSON body ({status}), got {e}: {}",
            String::from_utf8_lossy(&bytes)
        )
    });

    (String::from_utf8_lossy(&bytes).into_owned(), answer)
}
