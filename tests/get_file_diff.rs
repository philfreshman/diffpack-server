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

const INDEX_FROM: &str = "export function greet(name) {\n  return \"Hello, \" + name;\n}\n";

const INDEX_TO: &str = "export function greet(name) {\n  return \"Hi, \" + name;\n}\n";

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

const MAIN_FROM: &str = "function main() {\n\tlet x=1;\n}\n";

const MAIN_TO: &str = "function main() {\n    let x = 1;\n}\n";

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

#[tokio::test]
async fn changes_more_than_twice_the_context_apart_are_two_hunks() {
    let answer = patch(json!({
        "handle": handle("apart", "1.0.0", "2.0.0", false),
        "path": "src/far.js",
        "context_lines": 3,
    }))
    .await;

    assert_eq!(
        answer["text"],
        json!(
            "\
--- from/src/far.js
+++ to/src/far.js
@@ -2,7 +2,8 @@
  const before2 = 0;
  const before3 = 0;
  const before4 = 0;
- export const first = \"old\";
+ export const first = \"new\";
+ export const firstToo = \"new\";
  const between1 = 0;
  const between2 = 0;
  const between3 = 0;
@@ -10,7 +11,7 @@
  const between5 = 0;
  const between6 = 0;
  const between7 = 0;
- export const second = \"old\";
+ export const second = \"new\";
  const after1 = 0;
  const after2 = 0;
  const after3 = 0;"
        ),
        "seven unchanged lines between two changes is one more than the six \
         that three lines of context either side keep, so the middle one is \
         dropped and each change gets a header of its own, the second counted \
         from where it sits in each file: got {answer}"
    );
}

#[tokio::test]
async fn changes_exactly_twice_the_context_apart_share_one_hunk() {
    let answer = patch(json!({
        "handle": handle("apart", "1.0.0", "2.0.0", false),
        "path": "src/near.js",
        "context_lines": 3,
    }))
    .await;

    assert_eq!(
        answer["text"],
        json!(
            "\
--- from/src/near.js
+++ to/src/near.js
@@ -2,14 +2,15 @@
  const before2 = 0;
  const before3 = 0;
  const before4 = 0;
- export const first = \"old\";
+ export const first = \"new\";
+ export const firstToo = \"new\";
  const between1 = 0;
  const between2 = 0;
  const between3 = 0;
  const between4 = 0;
  const between5 = 0;
  const between6 = 0;
- export const second = \"old\";
+ export const second = \"new\";
  const after1 = 0;
  const after2 = 0;
  const after3 = 0;"
        ),
        "six unchanged lines between two changes is exactly the six that three \
         lines of context either side keep, so they stay one hunk with every \
         line between them: got {answer}"
    );
}

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

#[tokio::test]
async fn a_trimmed_answer_carries_no_line_the_full_answer_does_not() {
    for (package, path) in [
        ("renamey", "src/processor.js"),
        ("churny", "src/file00.js"),
        ("diffable", "src/index.js"),
        ("diffable", "src/added.js"),
        ("diffable", "src/removed.js"),
        ("reformatted", "src/main.js"),
        ("apart", "src/far.js"),
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

fn diffable() -> String {
    handle("diffable", "1.0.0", "2.0.0", false)
}

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

async fn patch(arguments: Value) -> Value {
    let result = call(arguments).await;

    assert_ne!(
        result["isError"],
        json!(true),
        "expected a patch, got a tool error: {result}"
    );

    result["structuredContent"].clone()
}

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

fn text(answer: &Value) -> &str {
    answer["text"]
        .as_str()
        .unwrap_or_else(|| panic!("a patch carries text, got {answer}"))
}

async fn listed(name: &str) -> Value {
    Client::fixture().listed(name).await
}

async fn call(arguments: Value) -> Value {
    Client::fixture().call(TOOL, arguments).await
}
