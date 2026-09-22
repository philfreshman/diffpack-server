//! The tools, held to the rules that are true of all of them.
//!
//! The seam under test is a tool module's interface, and it is deliberately
//! not reached directly: no test here calls `definitions()` or a handler.
//! Everything goes over the wire — `tools/list` for what a client is told —
//! because that is the only part a tool module is promising anything about. A
//! test that read the collection out of `src/tools/` would keep passing while
//! the definition a client is actually served stopped matching, which is the
//! failure #41 exists to prevent.
//!
//! What is new since then is the *over all of them*. There are eight tools,
//! and the rules below are collection-wide facts about them — a description
//! an agent can act on, a declared output shape, the hints a client decides
//! on, the registry enum coming from one module — so each is asserted once,
//! here, by walking what the server offers. A rule written per tool is a rule
//! the ninth tool is not held to, which is the whole reason a tool implements
//! a trait rather than being one of eight files that happen to look alike.
//!
//! [`TOOLS`] is the one thing in this file written by hand rather than read
//! from the wire, and that is the point: the `tools!` list in
//! `src/tools/mod.rs` and the list here are two sources that have to agree,
//! so a tool that arrives without a line here — or disappears without one
//! going — is a failure rather than a silently shorter loop.
//!
//! Each tool's own behaviour is its own suite: `tests/resolve_archive_url.rs`
//! and the seven beside it. `tests/mcp.rs` owns the transport, and
//! `tests/errors.rs` owns the two failure channels.

mod common;

use common::Client;
use serde_json::{json, Value};

/// The tools the `tools!` list in `src/tools/mod.rs` declares, in the order
/// the spec asks a server to list them: by name.
const TOOLS: [&str; 8] = [
    "diff_package_versions",
    "get_diff_tree",
    "get_file_content",
    "get_file_diff",
    "list_package_files",
    "list_package_versions",
    "resolve_archive_url",
    "search_packages",
];

// ---------------------------------------------------------------------------
// The collection
// ---------------------------------------------------------------------------

/// The tools a client is offered are the tools this file holds to the rules
/// below, and there are eight of them.
///
/// Every other test here loops over what `tools/list` returned, so without
/// this one a collection that had quietly become empty would satisfy all of
/// them.
#[tokio::test]
async fn the_collection_is_the_tools_the_list_declares() {
    assert_eq!(
        names(&Client::fixture().tools().await),
        TOOLS,
        "a tool added to `tools!` is a tool this file holds to the rules \
         below, and these two lists are what say so"
    );
}

/// Everything #23 asks a tool to carry, asked of every tool there is.
///
/// The point of one module per tool is that these come from the module rather
/// than from a second list, so a tool that arrives without them is a tool
/// that never compiled — and this is where that is checked rather than
/// assumed, over the collection rather than over whichever member somebody
/// remembered.
#[tokio::test]
async fn every_tool_is_listed_with_everything_an_agent_needs() {
    for tool in Client::fixture().tools().await {
        let name = named(&tool);

        assert!(
            tool["description"]
                .as_str()
                .is_some_and(|text| !text.is_empty()),
            "`{name}` is picked by an agent that has read nothing else, so it \
             needs a description: got {tool}"
        );

        assert_eq!(
            tool["inputSchema"]["type"], "object",
            "`{name}`'s input schema should be an object schema, got {}",
            tool["inputSchema"]
        );
        assert!(
            tool["inputSchema"]["properties"].is_object(),
            "`{name}`'s input schema should describe its arguments, got {}",
            tool["inputSchema"]
        );

        assert_eq!(
            tool["outputSchema"]["type"], "object",
            "`{name}` answers with structured content, so it has to declare \
             its shape: got {tool}"
        );
    }
}

/// The three hints, on every tool, because a client reads them before it
/// decides whether to ask a user first.
///
/// Two of them are the same answer for every tool here and are asserted as
/// such: this server only ever reads, and everything it reads about lives on
/// a registry it does not control. The third is the one that genuinely
/// differs — a search is not the same answer twice — so what is required here
/// is only that a tool states it, and each tool's own suite says what its
/// answer is and why.
#[tokio::test]
async fn every_tool_declares_the_hints_a_client_decides_on() {
    for tool in Client::fixture().tools().await {
        let name = named(&tool);
        let annotations = &tool["annotations"];

        assert_eq!(
            annotations["readOnlyHint"], true,
            "`{name}` changes nothing — no tool here does — and a client \
             deciding whether to ask for confirmation reads this: got \
             {annotations}"
        );
        assert_eq!(
            annotations["openWorldHint"], true,
            "`{name}` answers about a package on a registry, which is a world \
             this server does not control, got {annotations}"
        );
        assert!(
            annotations["idempotentHint"].is_boolean(),
            "`{name}` has to say whether asking twice gives the same answer; \
             a default would be a guess: got {annotations}"
        );
    }
}

/// `registry` is the enum `src/registry.rs` defines, wherever it appears.
///
/// The schema is where an agent learns what it may pass, so a tool that
/// spelled the list itself would be the fifth copy ADR 0004 rejects — and the
/// one an agent reads first. Over every tool that takes one rather than over
/// a named tool, because the copy that gets made is in whichever tool is
/// written next.
#[tokio::test]
async fn every_registry_argument_is_the_enum_the_registry_module_owns() {
    let mut asked = 0;

    for tool in Client::fixture().tools().await {
        let name = named(&tool);
        let registry = &tool["inputSchema"]["properties"]["registry"];
        if registry.is_null() {
            continue;
        }

        asked += 1;
        assert_eq!(
            registry["enum"],
            json!(["npm", "crates", "pypi"]),
            "`{name}` should list the registries this server has, got {registry}"
        );
    }

    assert!(
        asked > 0,
        "every tool but the ones that take a handle names a registry, so \
         finding none means this walked the wrong field"
    );
}

/// Nothing an agent reads names a Rust path.
///
/// Every `description` in a tool's definition reaches a model, and one saying
/// a field's "enum comes from [`crate::registry`]" hands it this repository's
/// reasoning rather than anything it can act on. The reasoning belongs beside
/// the code it is about; the schema belongs to the caller. #23 owns that.
///
/// Over the whole of each definition rather than the fields this file names
/// elsewhere: the description that leaks next is in a tool nobody has written
/// yet, in whatever shape its schema turns out to have.
#[tokio::test]
async fn no_description_an_agent_reads_names_a_rust_path() {
    let mut leaked = Vec::new();

    for tool in Client::fixture().tools().await {
        let name = named(&tool).to_owned();
        descriptions(&tool, &mut |said| {
            if said.contains("crate::") || said.contains("[`") {
                leaked.push(format!("{name}: {said}"));
            }
        });
    }

    assert!(
        leaked.is_empty(),
        "these reach a model and are written for us: {leaked:#?}"
    );
}

// ---------------------------------------------------------------------------
// Reading a definition
// ---------------------------------------------------------------------------

/// The names in a listing, in the order it gave them.
fn names(tools: &[Value]) -> Vec<&str> {
    tools.iter().map(named).collect()
}

/// What a listed tool calls itself, or a placeholder — so a failure can still
/// name the entry it was about.
fn named(tool: &Value) -> &str {
    tool["name"].as_str().unwrap_or("<unnamed>")
}

/// Every `description` anywhere in `value`, however deeply nested.
///
/// A walk rather than a list of places to look: a `description` on a field of
/// a type in `$defs` reaches a model exactly as one on a top-level property
/// does, and so will whatever nesting the next tool's schema has.
fn descriptions(value: &Value, found: &mut impl FnMut(&str)) {
    match value {
        Value::Object(fields) => {
            for (key, child) in fields {
                match (key.as_str(), child.as_str()) {
                    ("description", Some(said)) => found(said),
                    _ => descriptions(child, found),
                }
            }
        }
        Value::Array(items) => items.iter().for_each(|item| descriptions(item, found)),
        _ => {}
    }
}
