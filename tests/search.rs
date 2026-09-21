//! Which packages a registry has, asked of the module that fetches them.
//!
//! The counterpart of `tests/archive.rs` and `tests/catalogue.rs`, and it
//! holds what those suites hold: what this server will not read, and that the
//! cap it refuses by is set above the one source that is a whole document.
//!
//! What the *answer* looks like — the three shapes, the ranking, the empty
//! page — is `tests/search_packages.rs`, driven over the wire the way an
//! agent drives it. What is left for this file is the pair of refusals a tool
//! test cannot reach, because neither is something the fixture set can serve
//! through the tool's happy path.
//!
//! Every test below drives the fixture adapter, which reads
//! `fixtures/searches/` instead of a registry. The live adapter is exercised
//! in `tests/networked.rs`, which reaches the real sources and is `#[ignore]`d
//! for that reason.

use diffpack_server::error::Failure;
use diffpack_server::registry::Registry;
use diffpack_server::search::{self, Search};

// ---------------------------------------------------------------------------
// What this server will not read
// ---------------------------------------------------------------------------

/// An answer over the cap is refused, and refused as the failure that names
/// it rather than as the registry being unwell: the source answered, and went
/// on answering past what this server holds.
///
/// The cap is a value on the seam so that this can be asserted with a real
/// answer and a small limit, rather than by finding a registry that serves
/// 64 MB. It is applied where both adapters pass through, so the fixture set
/// cannot answer with something the registries would have been refused for —
/// which is what makes this test about the rule rather than about `fetch`.
#[tokio::test]
async fn a_search_answer_over_the_size_cap_is_refused() {
    let failure = fixtures()
        .with_limit(64)
        .hits(Registry::Crates, "serde", 200)
        .await
        .expect_err("64 bytes is smaller than any real answer");

    match failure {
        Failure::SearchTooLarge {
            registry,
            bytes,
            limit,
        } => {
            assert_eq!(registry, "crates.io");
            assert_eq!(limit, 64, "the refusal names the limit that was applied");
            assert!(
                bytes > 64,
                "the refusal names what was on offer, got {bytes} bytes"
            );
        }
        other => panic!("a refusal a model can act on, got {other:?}"),
    }
}

/// An answer this server cannot read is the registry's problem, and it is
/// neither a panic nor a query that matched nothing.
///
/// The two are worth keeping apart. A model told nothing matched tries
/// another word and keeps trying; a model told the answer would not read
/// searches somewhere else or uses a name it already has, which is what the
/// message says to do.
///
/// The fixture is npm's own shape with `objects` missing — the field the
/// whole answer is built from — rather than rubbish bytes, because that is
/// what a registry serving something unexpected actually looks like.
#[tokio::test]
async fn an_answer_this_server_cannot_read_is_not_a_query_that_matched_nothing() {
    let failure = fixtures()
        .hits(Registry::Npm, "unreadable", 200)
        .await
        .expect_err("the fixture answers in a shape this server does not know");

    match failure {
        Failure::UnreadableSearch { registry, reason } => {
            assert_eq!(registry, "npm");
            assert!(
                reason.contains("shape"),
                "the refusal should say what it objected to, got {reason}"
            );
        }
        other => panic!("an answer that will not read is not an empty page, got {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// The cap production runs with
// ---------------------------------------------------------------------------

/// The cap has to sit above the one source that is a whole document, or it
/// refuses every PyPI search rather than a large one.
///
/// PyPI's index was 44 MB when #19 measured it, and it grows with every
/// project published, so the cap is sized to leave that room rather than to
/// sit just above today's number.
#[test]
fn the_size_cap_is_above_the_index_pypi_publishes() {
    let the_index_today: u64 = 44_000_000;

    assert!(
        search::SIZE_LIMIT > the_index_today,
        "PyPI serves 44 MB and grows, got a cap of {} bytes",
        search::SIZE_LIMIT
    );
}

// ---------------------------------------------------------------------------
// Driving the seam
// ---------------------------------------------------------------------------

/// The adapter that reads `fixtures/searches/` rather than a registry.
fn fixtures() -> Search {
    Search::fixture(concat!(env!("CARGO_MANIFEST_DIR"), "/fixtures/searches"))
}
