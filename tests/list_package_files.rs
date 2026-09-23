//! `list_package_files`, driven the way an agent drives it.
//!
//! Two seams, because the tool promises two different things. Most of what is
//! here goes over the wire — `tools/list` for what a client is told,
//! `tools/call` for what it gets back — because a test that only called the
//! handler would keep passing while the definition beside it stopped
//! matching, which is the failure #41 exists to prevent. The handler is
//! reached directly only where the question is about the answer's *type*
//! rather than about its JSON.
//!
//! What is deliberately not re-proven here: that a walk of a sequence is
//! stable, that a page stays under the response ceiling by bytes, and that a
//! cursor of a client's own invention is refused. Those are `src/page.rs`'s
//! and `tests/page.rs` holds them against a generated sequence, which is a
//! stronger fixture than any package. What this suite asserts is that this
//! tool goes *through* that module rather than around it.
//!
//! Nor that no `description` an agent reads names a Rust path. That is a rule
//! every tool is held to rather than a fact about this one, so `tests/tools.rs`
//! holds it over the whole of `tools/list`. A copy here would have gone on
//! passing on the day a new tool leaked one.
//!
//! Extraction is `tests/archive.rs`'s the same way. The archives below are
//! the fixture set, so a fetch that built a URL of its own finds nothing.

mod common;

use common::{Client, FIXTURES};
use diffpack_server::error::Failure;
use diffpack_server::page;
use diffpack_server::registry::Registry;
use diffpack_server::tools::list_package_files::{Args, Entry, EntryType, ListPackageFiles};
use diffpack_server::tools::Ctx;
use diffpack_server::tools::Tool;
use serde_json::{json, Value};

const TOOL: &str = "list_package_files";

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

/// The one thing an agent cannot work out from a path it is shown: the
/// archive's top-level directory is already gone. An agent that does not know
/// will ask for `zod-4.0.0/src/index.js` and be told there is no such file.
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

/// `limit` and `cursor` carry `src/page.rs`'s numbers, not numbers this tool
/// wrote down. A tool that spelled out `limit: integer` with a sentence about
/// the default would be the one copy no test compares against `MAX_LIMIT`.
#[tokio::test]
async fn the_paging_arguments_document_the_numbers_that_bind() {
    let tool = listed(TOOL).await;
    let limit = &tool["inputSchema"]["properties"]["limit"];

    assert_eq!(limit["default"], json!(page::DEFAULT_LIMIT), "got {limit}");
    assert_eq!(limit["maximum"], json!(page::MAX_LIMIT), "got {limit}");
    assert_eq!(limit["minimum"], json!(1), "got {limit}");

    // The prose too, and this is the half a tool can take away without
    // noticing: a doc comment on the field overrides the description the type
    // wrote, and what is lost is the sentence saying an out-of-range `limit`
    // is clamped rather than refused. The numbers would still be right and
    // the agent would still be told the wrong thing.
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

// ---------------------------------------------------------------------------
// What it answers
// ---------------------------------------------------------------------------

/// The tracer bullet: one npm package, listed.
///
/// `@types/node` is wrapped in `package/` the way every npm tarball is, so
/// the paths below are also the assertion that the top-level directory is
/// gone — which is the whole of what makes two versions comparable, and the
/// one thing an agent cannot guess from a path it is shown.
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

/// crates.io wraps a crate in `{name}-{version}/`, which is a different
/// wrapper and a different extension from npm's and the same listing out the
/// other side. That it *is* the same is the point: a caller comparing two
/// registries' packages should not be reading two path conventions.
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

/// PyPI is two requests rather than one — the version's metadata, then the
/// artefact it names — and this tool makes neither of them. That the listing
/// comes back at all is the assertion: the second hop is `archive`'s, and a
/// tool that had to know PyPI needs asking would be carrying that module's
/// job around.
///
/// `requests` 2.31.0 publishes both a source distribution and a wheel, and
/// `setup.py` is only in the sdist — so which artefact was chosen is visible
/// here rather than only in a URL nobody sees.
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

/// An entry says what it is and how big it is, not only where it is.
///
/// The type is the part an agent cannot infer: `src` and `src/lib.rs` are
/// both paths, and only one of them has content to ask for. The sizes are
/// the archive's own, read out of it with `tar` when this test was written
/// rather than out of this server — an expected value produced the same way
/// the code produces it would agree with a bug.
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

// ---------------------------------------------------------------------------
// Narrowing it
// ---------------------------------------------------------------------------

/// `prefix` answers "what is in this directory", which is the question an
/// agent asks second. The directory itself is not in its own subtree: an
/// agent that asked what is in `src/` is not asking to be told about `src`.
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

/// The "and nothing else" half, and the one a string comparison gets wrong.
/// A prefix names a directory, so `sr` is not a shorter way of writing `src`
/// — it is a directory this package does not have. An implementation that
/// matched on characters would answer with `src/lib.rs` here and be wrong in
/// a way nobody notices until a package has both `lib` and `libs`.
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

/// A trailing slash is how half the world spells a directory, and an agent
/// that wrote one should not get a different answer from one that did not.
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

/// `/` is the root, and the root's subtree is the whole archive.
///
/// It used to be an empty page with a total of nought: the slash was trimmed
/// and then put back, which named a directory called `/` that no path is
/// under. `get_diff_tree` already read `/` as the root, so the two tools that
/// take a directory answered one question two ways.
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

/// An empty prefix is the root too, for the reason `/` is: nothing is what a
/// trailing slash leaves of `/`, so the two are one argument.
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

// ---------------------------------------------------------------------------
// Reading it a page at a time
// ---------------------------------------------------------------------------

/// That a walk is stable — every path once, none missed, none repeated — is
/// `src/page.rs`'s property and `tests/page.rs` proves it against a
/// generated sequence. What is left for this tool is the part only it can be
/// wrong about: that the sequence reaches that module at all, and that the
/// cursor it hands back is the one that resumes the walk.
///
/// A listing cut by `limit` therefore has to state the *archive's* total
/// rather than the page's, and the concatenation of the pages has to be the
/// whole listing.
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

/// A cursor is opaque and this tool declares it as `page::Cursor`, so one a
/// client wrote for itself is refused before the handler runs. The rule
/// reaches an agent through the schema rather than through a sentence this
/// tool wrote, which is why it is not restated here — what is asserted is
/// that the tool inherited it rather than declaring a `String`.
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

/// A package with thousands of files comes back as a page, and the page fits
/// in a response the platform will carry.
///
/// This is the criterion no small package can fail, and the shape a naive
/// implementation gets wrong: returning every entry is correct for `serde`
/// and a platform error — a `500` with nothing in it a client can read — for
/// anything real. What is asserted is that the answer is bounded, that the
/// total is the archive's rather than the page's, and that there is a cursor
/// for the rest.
///
/// It is deliberately not a re-proof of the byte ceiling. That one is on
/// serialised bytes and bites when items are large rather than when they are
/// many; `tests/page.rs` holds it against items sized to reach it, which a
/// listing of ordinary paths never does. Here the whole response is measured
/// against `RESPONSE_CEILING` because that is the number the platform
/// enforces, and a page asking for the largest `limit` there is, is the most
/// this tool can ever be asked to answer with.
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

/// A `FileMap` is a map, so the sequence a cursor names a position in has to
/// be imposed rather than inherited. Two calls asking for the same page have
/// to be the same page — otherwise a walk of a large package reads some
/// entries twice and never reaches others, and nothing in the answer says so.
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

// ---------------------------------------------------------------------------
// How it fails
// ---------------------------------------------------------------------------

/// A version the registry does not have is something the model can act on —
/// by asking for one that exists — so it is a tool error rather than a
/// protocol one, and it names what was not found. A model told only that a
/// request failed has no next call to make.
///
/// The fixture index carries this URL as one that serves nothing, so the
/// answer is the one a real `404` produces without a real `404`.
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

/// A package no registry has fails the same way and for the same reason: the
/// status a registry answers with does not say which half of the pair was
/// wrong, and guessing would send a model to check the version when the name
/// is what it mistyped.
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

// ---------------------------------------------------------------------------
// The handler, reached directly
// ---------------------------------------------------------------------------
//
// The second seam, and a narrow one on purpose. Everything above goes over
// the wire because that is where a definition and a handler can disagree.
// What is left for these two is the part JSON cannot show: which `Failure`
// the handler returned, and that the answer is a `Page<Entry>` of typed
// values rather than a shape that happens to serialise to the right JSON.

/// The handler answers in the crate's own types.
///
/// `Entry` is what the output schema is generated from, so a field that
/// serialises correctly by accident — a stringly-typed `size`, a `type` that
/// is any string at all — would pass every test above and fail a client that
/// validated against the schema.
#[tokio::test]
async fn the_handler_answers_with_typed_entries() {
    let page = ListPackageFiles::call(
        Args {
            registry: Registry::Crates,
            package: "serde".to_owned(),
            version: "1.0.0".to_owned(),
            prefix: Some(page::Subtree::new("src")),
            cursor: None,
            limit: None,
        },
        &Ctx::fixture(FIXTURES),
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

/// Which failure it is, rather than which words it produced.
///
/// The test over the wire asserts that the message names the version, which
/// is what a model reads. This asserts the variant, which is what decides the
/// channel it goes out on — and a handler that returned the right words on
/// the wrong variant would be a `404` reported as a protocol error the model
/// never sees.
#[tokio::test]
async fn the_handler_returns_the_failure_that_names_what_was_not_found() {
    let failure = ListPackageFiles::call(
        Args {
            registry: Registry::Npm,
            package: "zod".to_owned(),
            version: "99.99.99".to_owned(),
            prefix: None,
            cursor: None,
            limit: None,
        },
        &Ctx::fixture(FIXTURES),
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

// ---------------------------------------------------------------------------
// Driving the endpoint
// ---------------------------------------------------------------------------

/// The `path` of every entry on this page, in the order they were returned.
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

/// The listed definition of `name`, or a panic naming what was listed.
async fn listed(name: &str) -> Value {
    Client::fixture().listed(name).await
}

/// Call this tool with `arguments`, returning the `result`.
async fn call(arguments: Value) -> Value {
    Client::fixture().call(TOOL, arguments).await
}
