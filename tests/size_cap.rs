//! What this server will not hold, asked of all three seams at once.
//!
//! The cap is one rule — *the most one downloaded body may weigh before this
//! server refuses it unread* — and since #81 it is one implementation, under
//! `archive`, `catalogue` and `search` alike. So it is one test rather than
//! the three near-copies that stood in `tests/archive.rs`,
//! `tests/catalogue.rs` and `tests/search.rs`, each driving the same
//! comparison through a different seam.
//!
//! What is still three is the *refusal*. A model told an archive was too
//! large has a smaller thing to ask for — one file instead of a whole tree —
//! and a model told a version list was has nothing smaller to ask for at all,
//! so the three seams name their own and this test holds them to it. That is
//! the half of the shared module's interface worth pinning here: it takes its
//! refusals as parameters, and a parameter that all three callers had to pass
//! the same value for would not have been worth taking.
//!
//! Every seam below is driven through its own public interface against the
//! checked-in fixture sets, with a limit of 64 bytes rather than a package
//! nobody wants to download in a test. The cap is a value on each seam for
//! exactly that reason.
//!
//! What this file does *not* show is that the cap is enforced *once*. Two
//! checks with one number and one constructor answer a caller identically, so
//! no call through any of these interfaces can tell them apart; that half is
//! read in `src/document/mod.rs`, where the one comparison is, and it is the
//! reason the redundant per-seam weighing was removed rather than left as a
//! belt to the braces.

use diffpack_server::archive::Archive;
use diffpack_server::catalogue::Catalogue;
use diffpack_server::error::Failure;
use diffpack_server::registry::Registry;
use diffpack_server::search::Search;

/// A body over the cap is refused, whichever seam asked for it — and refused
/// in that seam's own words.
///
/// Three assertions and one rule. Each seam hands the shared module a
/// different `too_large`, so what this drives is both that the weighing
/// happens for all three and that none of them was folded into a single
/// refusal on the way.
///
/// The sizes are asserted as *over the limit* rather than as a number: the
/// fixtures are real archives and real documents, and pinning their byte
/// counts here would make repacking one a failing test in a file that is not
/// about them.
#[tokio::test]
async fn a_body_over_the_cap_is_refused_in_the_words_of_the_seam_that_asked() {
    let archive = Archive::fixture(set("archives"))
        .with_limit(64)
        .fetch(Registry::Npm, "@types/node", "20.1.0")
        .await
        .expect_err("64 bytes is smaller than any real archive");

    match archive {
        Failure::TooLarge {
            package,
            version,
            bytes,
            limit,
        } => {
            assert_eq!(package, "@types/node");
            assert_eq!(version, "20.1.0");
            assert_eq!(limit, 64, "the refusal names the limit that was applied");
            assert!(
                bytes > 64,
                "the refusal names what was on offer, got {bytes} bytes"
            );
        }
        other => panic!("an archive over the cap is `too_large`, got {other:?}"),
    }

    let versions = Catalogue::fixture(set("versions"))
        .with_limit(64)
        .versions(Registry::Crates, "tokio")
        .await
        .expect_err("64 bytes is smaller than any real document");

    match versions {
        Failure::VersionsTooLarge {
            registry,
            package,
            bytes,
            limit,
        } => {
            assert_eq!(registry, "crates.io");
            assert_eq!(package, "tokio");
            assert_eq!(limit, 64, "the refusal names the limit that was applied");
            assert!(
                bytes > 64,
                "the refusal names what was on offer, got {bytes} bytes"
            );
        }
        other => panic!("a version list over the cap is `versions_too_large`, got {other:?}"),
    }

    let hits = Search::fixture(set("searches"))
        .with_limit(64)
        .hits(Registry::Crates, "serde", 200)
        .await
        .expect_err("64 bytes is smaller than any real answer");

    match hits {
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
        other => panic!("a search answer over the cap is `search_too_large`, got {other:?}"),
    }
}

/// One of the checked-in fixture sets, by the directory it lives in.
fn set(name: &str) -> String {
    format!("{}/fixtures/{name}", env!("CARGO_MANIFEST_DIR"))
}
