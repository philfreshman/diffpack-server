//! `get_file_content`, driven the way an agent drives it.
//!
//! Two seams, the same two `tests/list_package_files.rs` uses and for the
//! same reason. Most of what is here goes over the wire — `tools/list` for
//! what a client is told, `tools/call` for what it gets back — because a test
//! that only called the handler would keep passing while the definition
//! beside it stopped matching, which is the failure #41 exists to prevent.
//! The handler is reached directly only where the question is about the
//! answer's *type* rather than about its JSON.
//!
//! What is deliberately not re-proven here: where a cut falls, what the
//! marker says, and that the ceiling is measured on serialised bytes rather
//! than on length. Those are `src/page.rs`'s and `tests/page.rs` holds them
//! against generated text, which is a stronger fixture than any file in a
//! package. What this suite asserts is that this tool goes *through* that
//! module rather than around it.
//!
//! Nor that no `description` an agent reads names a Rust path — that is a
//! rule every tool is held to rather than a fact about this one, so
//! `tests/tools.rs` holds it over the whole of `tools/list`.
//!
//! Extraction, and the lossy decoding a file that is not UTF-8 goes through,
//! are `tests/archive.rs`'s. The archives below are the fixture set, so a
//! fetch that built a URL of its own finds nothing.

mod common;

use common::{Client, FIXTURES};
use diffpack_server::error::Failure;
use diffpack_server::page::{self, Excerpt};
use diffpack_server::registry::Registry;
use diffpack_server::tools::get_file_content::{Args, GetFileContent};
use diffpack_server::tools::Ctx;
use diffpack_server::tools::Tool;
use serde_json::{json, Value};

const TOOL: &str = "get_file_content";

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

/// The three things the engine's behaviour forces into the description,
/// because an agent that does not know them draws a wrong conclusion from a
/// correct answer.
///
/// A path has no top-level directory, so an agent that writes one back gets
/// nothing. A directory is refused rather than answered with an empty
/// string. And a file that is not UTF-8 comes back as replacement characters
/// rather than as a failure — which reads as a corrupt file to anyone who
/// was not told.
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

/// `max_bytes` carries the response module's number, not one this tool wrote
/// down. A tool spelling out `max_bytes: integer` with a sentence of its own
/// would be the one copy no test compares against the ceiling — and the
/// sentence it would lose is the one saying a larger value is narrowed
/// rather than refused.
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

// ---------------------------------------------------------------------------
// What it answers
// ---------------------------------------------------------------------------

/// The tracer bullet: one file out of one npm package, exactly as it was
/// packed.
///
/// The expected text is the fixture's own, written by hand and packed by
/// `tar` when `scripts/make-archive-fixtures.sh` was written — not read back
/// out of this server, which would agree with a bug.
///
/// The path is `package.json` rather than `package/package.json`: the
/// archive's top-level directory is stripped before a path is ever asked
/// for, which is the one thing about a path an agent cannot infer.
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

/// A crate's file, out of a different wrapper and a different extension and
/// through the same path.
///
/// Green the moment the npm one was, because the fetch path is the same code
/// for all three registries — which is the thing being asserted. A tool that
/// had learned anything registry-shaped on its way to a file would be the
/// one to fail here.
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

/// PyPI is two requests rather than one — the version's metadata, then the
/// artefact it names — and this tool makes neither of them. That a file
/// comes back at all is the assertion: the second hop belongs to the module
/// that fetches, and a tool that had to know PyPI needs asking would be
/// carrying that module's job around.
///
/// `setup.py` is in the source distribution and not in the wheel, so which
/// artefact was chosen is visible in the text rather than only in a URL
/// nobody sees.
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

// ---------------------------------------------------------------------------
// Cutting it
// ---------------------------------------------------------------------------

/// A cut has to be loud. A silently shortened file is how an agent concludes
/// a function does not exist — it read what it was given, found no
/// `serialize`, and had nothing in the answer to tell it the file went on.
///
/// So all three are asserted together: the text that came back, that it says
/// it was cut, and that the byte count is the *file's* and not the excerpt's.
/// A tool reporting the returned length there would be telling an agent that
/// a file it has seen a fifth of is a fifth long.
///
/// 51 is the fixture's own size, from `wc -c` on the archive `tar` packed —
/// not from this server, which would agree with a bug.
///
/// Where the cut falls and what the marker says are `src/page.rs`'s, and
/// `tests/page.rs` holds them. What is asserted here is that this tool goes
/// through that module: it keeps what was asked for, and it does not invent
/// a count of its own.
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

/// A file that fits comes back whole and says so, which is the half that
/// stops `truncated` from being decoration: an agent that saw it set on
/// every answer would learn to ignore it.
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

/// A file larger than a whole response comes back cut, with no `max_bytes`
/// asked for — "the server applies a default regardless", which is the case
/// a caller cannot reach by asking nicely and the one a naive implementation
/// gets wrong. Returning the file is correct for `package.json` and a
/// platform error for anything real: a body over the cap is not a long
/// answer, it is a `500` with nothing in it a client can read.
///
/// 2,000,000 is the fixture's own size, from `wc -c` on the archive `tar`
/// packed. What is asserted beside it is that the framed response fits in
/// what the platform will carry — measured against the ceiling rather than
/// against a number this test chose, because that is the one the platform
/// enforces.
///
/// Where the cut falls is not re-proven here; `tests/page.rs` holds that
/// against generated text of every escaping cost. What is asserted is that
/// this tool reaches that module with no prompting.
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

// ---------------------------------------------------------------------------
// Text that was never text
// ---------------------------------------------------------------------------

/// A file that is not valid UTF-8 comes back decoded lossily, which is what
/// the extractor already does to it, and the answer says so.
///
/// Not an error, because a binary file in a package is an ordinary thing and
/// an agent asking about one has not made a mistake. But without the flag it
/// is handed a string of replacement characters and cannot tell a PNG from a
/// source file somebody saved in the wrong encoding — and those have
/// different next moves.
///
/// The expected text is what `python3 -c` said the fixture's ten bytes decode
/// to, not what this server said: a PNG signature whose first byte stands
/// alone, then two bytes that are never valid. Three replacement characters,
/// and `PNG` still legible between them.
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

/// The other half, and the one that keeps the flag from being decoration: an
/// ordinary source file says it decoded cleanly. A flag set on every answer
/// is a flag an agent learns to skip.
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

/// An empty file is a file, and answers as one: no text, nothing cut, and
/// clean. The extractor gives a directory the same empty string, so a tool
/// that told the two apart by emptiness would refuse this as a directory —
/// and an agent told `empty.txt` is a directory has been told something false
/// about what the package ships.
///
/// The size is the fixture's own, from `tar tv` on the archive, not from
/// this server.
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

// ---------------------------------------------------------------------------
// How it fails
// ---------------------------------------------------------------------------

/// A directory has no content, and saying so is the whole point: the
/// extractor gives a directory the empty string, so a tool that passed that
/// through would tell an agent that `src` is a file with nothing in it. The
/// agent's next move — conclude the package ships an empty module — is wrong
/// and it has nothing to notice it with.
///
/// It is a tool error rather than a protocol one because the model is who
/// can fix it, by asking for a file inside the directory instead.
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

/// A path the version does not have is something the model can act on — by
/// listing the version's files and asking for one of those — so it takes the
/// channel the model reads, and it names what was not found. A model told
/// only that a request failed has no next call to make.
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

// ---------------------------------------------------------------------------
// The handler, reached directly
// ---------------------------------------------------------------------------
//
// The second seam, and a narrow one on purpose. Everything above goes over
// the wire because that is where a definition and a handler can disagree.
// What is left for these three is the part JSON cannot show: which `Failure`
// the handler returned, and that the answer is a typed value rather than a
// shape that happens to serialise to the right JSON.

/// The handler answers in the crate's own types.
///
/// The excerpt is the response module's, not three fields this tool spelled
/// for itself — a `bytes` that serialised as a string, or a `truncated` that
/// was any value at all, would pass every test above and fail a client that
/// validated against the schema.
///
/// 22 is the fixture's own size: twenty-one characters and a newline, from
/// the line `scripts/make-archive-fixtures.sh` writes.
#[tokio::test]
async fn the_handler_answers_with_a_typed_excerpt() {
    let content = GetFileContent::call(
        Args {
            registry: Registry::Crates,
            package: "serde".to_owned(),
            version: "1.0.0".to_owned(),
            path: "src/lib.rs".to_owned(),
            max_bytes: None,
        },
        &Ctx::fixture(FIXTURES),
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

/// Which failure a directory is, rather than which words it produced.
///
/// The test over the wire asserts the message names the path, which is what
/// a model reads. This asserts the variant, which is what decides the
/// channel it goes out on — and a handler that produced the right words on
/// the wrong variant would send a directory down the protocol channel, where
/// the model never sees it and cannot ask for a file inside instead.
#[tokio::test]
async fn the_handler_returns_the_failure_that_says_the_path_is_a_directory() {
    let failure = GetFileContent::call(
        Args {
            registry: Registry::Crates,
            package: "serde".to_owned(),
            version: "1.0.0".to_owned(),
            path: "src".to_owned(),
            max_bytes: None,
        },
        &Ctx::fixture(FIXTURES),
    )
    .await
    .expect_err("`src` is a directory of this crate's");

    match failure {
        Failure::PathIsDirectory { path, .. } => assert_eq!(path, "src"),
        other => panic!("a directory should say so, got {other:?}"),
    }
}

/// And the other way a path is wrong, which is a different variant because
/// it is a different remedy: not "ask for a file inside this" but "find out
/// what the paths are". A handler collapsing the two would leave a model
/// guessing which one it is looking at.
#[tokio::test]
async fn the_handler_returns_the_failure_that_names_the_absent_path() {
    let failure = GetFileContent::call(
        Args {
            registry: Registry::Crates,
            package: "serde".to_owned(),
            version: "1.0.0".to_owned(),
            path: "src/nowhere.rs".to_owned(),
            max_bytes: None,
        },
        &Ctx::fixture(FIXTURES),
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

// ---------------------------------------------------------------------------
// Driving the endpoint
// ---------------------------------------------------------------------------

/// The listed definition of `name`, or a panic naming what was listed.
async fn listed(name: &str) -> Value {
    Client::fixture().listed(name).await
}

/// Call this tool with `arguments`, returning the `result`.
async fn call(arguments: Value) -> Value {
    Client::fixture().call(TOOL, arguments).await
}
