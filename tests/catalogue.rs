//! What a package has released, asked of the module that fetches it.
//!
//! The counterpart of `tests/archive.rs`, and it holds what that suite holds:
//! a refusal a tool test cannot reach, and that the cap this seam refuses by
//! is set where a real document fits under it.
//!
//! What the *answer* looks like — newest first, the dates, the preview flag —
//! is `tests/list_package_versions.rs`, driven over the wire the way an agent
//! drives it. What is left for this file is the refusal that a tool test
//! cannot reach, because it is not a document the fixture set can serve
//! through the tool's happy path.
//!
//! The cap *firing* is not here and is not missing. It is one rule under all
//! three seams since #81, so it is driven once, in `tests/size_cap.rs`, which
//! holds this seam to naming its own refusal when it does.
//!
//! Every test below drives the fixture adapter, which reads
//! `fixtures/versions/` instead of a registry. The live adapter is exercised
//! in `tests/networked.rs`, which reaches the real sources and is `#[ignore]`d
//! for that reason.

use diffpack_server::catalogue::{self, Catalogue};
use diffpack_server::error::Failure;
use diffpack_server::registry::Registry;

// ---------------------------------------------------------------------------
// What this server will not read
// ---------------------------------------------------------------------------

/// A document this server cannot read is the registry's problem and not the
/// caller's, and it is neither a panic nor a package that does not exist.
///
/// The two are worth keeping apart. A model told the package is missing tries
/// another name and keeps trying; a model told the answer would not read
/// stops asking this question and uses a version it already knows, which is
/// what the message says to do.
///
/// The fixture is npm's own shape with `versions` missing — the field the
/// whole answer is built from — rather than rubbish bytes, because that is
/// what a registry serving something unexpected actually looks like.
#[tokio::test]
async fn a_document_this_server_cannot_read_names_the_package_and_not_a_version() {
    let failure = fixtures()
        .versions(Registry::Npm, "unreadable")
        .await
        .expect_err("the fixture names no versions for this package");

    match failure {
        Failure::UnreadableVersions {
            registry,
            package,
            reason,
        } => {
            assert_eq!(registry, "npm");
            assert_eq!(package, "unreadable");
            assert!(
                reason.contains("shape"),
                "the refusal should say what it objected to, got {reason}"
            );
        }
        other => panic!("a document that will not read is not a missing package, got {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// The cap production runs with
// ---------------------------------------------------------------------------

/// The cap has to sit above what a real version document weighs, or it
/// refuses the packages people most want listed. npm's full metadata carries
/// every version's manifest, so the largest are the ones with the most
/// releases: `typescript`'s is 15.7 MB and `@types/node`'s is 10.6 MB.
///
/// A cap below those would fail on exactly the packages this tool exists for,
/// and it would fail with a message saying the server declined to read them
/// rather than with anything a caller could act on.
#[test]
fn the_size_cap_is_above_what_a_large_version_document_weighs() {
    let a_large_document: u64 = 16_000_000;

    assert!(
        catalogue::SIZE_LIMIT > a_large_document,
        "npm serves 15.7 MB for `typescript`, got a cap of {} bytes",
        catalogue::SIZE_LIMIT
    );
}

// ---------------------------------------------------------------------------
// Driving the seam
// ---------------------------------------------------------------------------

/// The adapter that reads `fixtures/versions/` rather than a registry.
fn fixtures() -> Catalogue {
    Catalogue::fixture(concat!(env!("CARGO_MANIFEST_DIR"), "/fixtures/versions"))
}
