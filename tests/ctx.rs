//! The context a test builds.
//!
//! Every other suite here builds a `Ctx` to drive one tool with, and reads
//! that tool's answer. What none of them reads is the context itself: which
//! adapter each of its seams got. A seam a suite does not use is a seam
//! nothing asserts anything about, and for as long as its tool does not exist
//! that is invisible — until the tool arrives, reaches the seam through a
//! context built for a different one, and asks a registry from CI.
//!
//! So this suite drives the seams a context carries rather than a tool: the
//! shortest call that reaches each one, chosen for that and not because this
//! is a suite about the tool making it. Each goes over the wire, through the
//! service factory `router_with` takes, because building a context is only
//! interesting if it is the context a handler is handed — and every call for
//! one seam goes to one context, because a seam that remembers between calls
//! has nothing to say to a context built fresh for each.
//!
//! Which seams those are is `Ctx::seams`'s answer and not this file's list,
//! so a seam added to a context with no call written for it fails here rather
//! than waiting for the day it is live.

use std::path::Path;
use std::time::Duration;

mod common;

use common::{Client, FIXTURES};
use diffpack_server::tools::Ctx;
use serde_json::{json, Value};

/// A root with nothing under it.
///
/// The fixture adapters answer a directory they cannot read with
/// [`diffpack_server::error::Failure::Internal`], which is the refusal this
/// suite is after: a live adapter has no way to produce it, so a seam that
/// answers with it is a seam that never left this machine.
const NO_FIXTURES: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/fixtures/no-set-is-checked-in-here"
);

/// One seam a context carries, and the shortest call that reaches it.
struct Seam {
    /// What [`Ctx::seams`] calls it.
    name: &'static str,

    tool: &'static str,
    arguments: Value,

    /// How many times to make that call before reading the answer.
    ///
    /// One for every seam whose answer is the document it read. The store's
    /// is two, because a cache has nothing to say about the first call: what
    /// distinguishes this process's own store from one reaching outside is
    /// that the second call was served by the first.
    calls: usize,

    /// Where in that tool's answer to read, and what the fixture set says
    /// there — a fact the registry this seam stands in for could not have
    /// answered with.
    reads: &'static str,
    fixture_says: Value,

    /// What the fixture adapter says it was doing when it could not find its
    /// set, which is how a refusal is known to have come from disk.
    ///
    /// `None` for the store, and that is the rule rather than an exception
    /// to it: a cache failure must never reach a caller (ADR 0003), so a
    /// store that refused would be the bug and not the proof. What stands in
    /// for this check there is the assertion above — a store this test can
    /// make answer `cached` is a store inside this process.
    doing: Option<&'static str>,
}

/// Every seam, in the order a context carries them.
///
/// Adding one here is the second half of adding one to `Ctx`: the first half
/// is the compile error in both of its constructors.
fn seams() -> Vec<Seam> {
    vec![
        Seam {
            name: "archive",
            tool: "get_file_content",
            arguments: json!({
                "registry": "crates",
                "package": "serde",
                "version": "1.0.0",
                "path": "src/lib.rs",
            }),
            calls: 1,
            reads: "/text",
            // The fixture `serde` carries a `src/lib.rs` of one line that the
            // real crate does not.
            fixture_says: json!("pub fn serialize() {}\n"),
            doing: Some("reading the archive fixtures"),
        },
        Seam {
            name: "catalogue",
            tool: "list_package_versions",
            arguments: json!({ "registry": "npm", "package": "zod" }),
            calls: 1,
            reads: "/versions/total",
            // The fixture `zod` has four versions where the real package has
            // hundreds.
            fixture_says: json!(4),
            doing: Some("reading the version fixtures"),
        },
        Seam {
            name: "search",
            tool: "search_packages",
            arguments: json!({ "registry": "npm", "query": "zod" }),
            calls: 1,
            reads: "/total",
            // npm answers this query with hundreds; the fixture set answers
            // it with two.
            fixture_says: json!(2),
            doing: Some("reading the search fixtures"),
        },
        Seam {
            name: "store",
            tool: "diff_package_versions",
            arguments: json!({
                "registry": "npm",
                "package": "diffable",
                "from_version": "1.0.0",
                "to_version": "2.0.0",
            }),
            calls: 2,
            reads: "/cached",
            // The store a fixture context carries keeps its blobs in this
            // process, so the second call is served by the first. A context
            // left with a live store would answer `false` both times in CI,
            // where there are no blob credentials to reach one with.
            fixture_says: json!(true),
            doing: None,
        },
    ]
}

/// Every seam a context carries has a call here.
///
/// The list above is held to `Ctx::seams`, which is the struct's own answer,
/// so the next seam cannot arrive without one. Without this the two tests
/// below would keep passing while saying nothing about it, which is the shape
/// of the bug this suite exists for one level up.
#[test]
fn every_seam_a_context_carries_is_driven_here() {
    let mut driven: Vec<&str> = seams().iter().map(|seam| seam.name).collect();
    driven.sort_unstable();

    let mut carried = Ctx::fixture(FIXTURES).seams().to_vec();
    carried.sort_unstable();

    assert_eq!(
        driven, carried,
        "a seam with no call here is a seam nothing holds to the fixture \
         set: give it the shortest call that reaches it"
    );
}

/// Every seam is the fixture set's, and the right part of it.
///
/// Each call asserts something the registry it stands in for could not answer
/// with. So this fails if a seam was wired to another seam's directory, and
/// it fails if a seam was left live and the machine happened to have a
/// network.
#[tokio::test]
async fn every_seam_a_context_carries_is_the_fixture_set() {
    for seam in seams() {
        let answer = call(FIXTURES, &seam).await;
        let read = answer["result"]["structuredContent"].pointer(seam.reads);

        assert_eq!(
            read,
            Some(&seam.fixture_says),
            "the `{}` seam should have read the fixture set, got {answer}",
            seam.name
        );
    }
}

/// And none of them can reach a registry.
///
/// A context over a root that holds nothing: every seam refuses with the
/// failure its fixture adapter produces when it cannot read its set, which is
/// the internal channel and `-32000`. A seam the constructor had left live
/// would answer here instead, or fail naming somebody else's server.
///
/// This is what the two builders it replaced could not promise. Each filled
/// the seam it was not given from the live constructor, so `Ctx` over a
/// fixture archive carried a live catalogue: the first tool to read a
/// catalogue through one would have made a real request, in CI,
/// intermittently, and reported it as the registry being unreachable.
#[tokio::test]
async fn no_seam_a_context_carries_can_reach_a_registry() {
    assert!(
        !Path::new(NO_FIXTURES).exists(),
        "`{NO_FIXTURES}` has to hold nothing for this test to mean anything, \
         and something is checked in there"
    );

    for seam in seams() {
        // The store is the seam with nothing to refuse with. See `Seam`.
        let Some(doing) = seam.doing else {
            continue;
        };

        let answer = call(NO_FIXTURES, &seam).await;

        assert_eq!(
            answer["error"]["code"], -32000,
            "the `{}` seam should have failed reading a fixture set that is \
             not there, got {answer}",
            seam.name
        );
        assert_eq!(
            answer["error"]["message"],
            format!("diffpack failed while {doing}."),
            "the `{}` seam should have gone to the fixture adapter, got \
             {answer}",
            seam.name
        );
    }
}

/// `seam`'s call, against a server whose context is built over `fixtures`,
/// answering with the last of them.
///
/// One context for all of a seam's calls, cloned into each request. A `Ctx`
/// shares its seams through an `Arc`, which is what production does too: the
/// factory is what a request goes through, not what an adapter is rebuilt by.
/// Building a fresh one per request would give the store a fresh set of
/// blobs and make its second call indistinguishable from its first.
///
/// Answers the whole envelope rather than the result, because half of what is
/// asserted above is a JSON-RPC error and the other half is a result.
async fn call(fixtures: &'static str, seam: &Seam) -> Value {
    let client = Client::over(Ctx::fixture(fixtures));

    let mut answer = Value::Null;
    for request in 0..seam.calls {
        answer = client
            .post(json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "tools/call",
                "params": { "name": seam.tool, "arguments": seam.arguments },
            }))
            .await;

        // A seam is allowed to leave work running after it has answered, and
        // one does: the cache writes its entry after the response, which is
        // the whole of what `waitUntil` is for. So a call that follows
        // another gives it a moment rather than racing it.
        if request + 1 < seam.calls {
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }
    answer
}
