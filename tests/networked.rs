//! The live adapter, against the registries themselves.
//!
//! Every test here reaches npm, crates.io or PyPI over the network, so every
//! one is `#[ignore]`d: `cargo test` stays offline and deterministic, and
//! these are run deliberately.
//!
//! ```text
//! cargo test --test networked -- --ignored
//! ```
//!
//! What they are for is the half `tests/archive.rs` cannot state. That suite
//! proves what this server does with an archive; this one proves that the
//! URLs it builds are the URLs these three registries actually serve — which
//! is a fact about somebody else's server and can only be checked by asking
//! it. A registry that moved a path would fail here and nowhere else.

use diffpack_server::archive::Archive;
use diffpack_server::catalogue::Catalogue;
use diffpack_server::error::Failure;
use diffpack_server::registry::{Registry, Version};

/// npm, end to end: the URL `resolve_archive_url` answers with is a URL npm
/// serves, and what comes back is a package. The scoped name is the case
/// worth spending a real request on — the path keeps the scope and the
/// filename drops it, and a mistake there is a 404 for every `@types/*`
/// package there is.
#[tokio::test]
#[ignore = "networked: fetches from registry.npmjs.org"]
async fn npm_serves_the_archive_this_server_asks_it_for() {
    let files = Archive::live()
        .fetch(Registry::Npm, "@types/node", "20.1.0")
        .await
        .expect("npm serves this version");

    assert!(
        files.contains_key("package.json"),
        "every npm package has a `package.json` at its root"
    );
}

/// crates.io, end to end, from the static host rather than the API one.
#[tokio::test]
#[ignore = "networked: fetches from static.crates.io"]
async fn crates_io_serves_the_archive_this_server_asks_it_for() {
    let files = Archive::live()
        .fetch(Registry::Crates, "serde", "1.0.0")
        .await
        .expect("crates.io serves this version");

    assert!(
        files.contains_key("Cargo.toml"),
        "every crate carries a `Cargo.toml` at its root"
    );
}

/// PyPI, end to end, which is two requests: the version's metadata, then the
/// artefact it names. That the second URL is one PyPI serves is the part only
/// a real request can settle — it is chosen from a document rather than built.
///
/// `requests` 2.31.0 publishes both an sdist and a wheel, so it is also where
/// the preference is checked against the real listing rather than a fixture's.
#[tokio::test]
#[ignore = "networked: fetches from pypi.org and files.pythonhosted.org"]
async fn pypi_serves_the_source_distribution_it_lists() {
    let files = Archive::live()
        .fetch(Registry::PyPi, "requests", "2.31.0")
        .await
        .expect("PyPI lists and serves this version");

    assert!(
        files.contains_key("setup.py"),
        "the source distribution is the one to diff, and it carries `setup.py`"
    );
    assert!(
        !files.keys().any(|path| path.contains(".dist-info/")),
        "`.dist-info/` means the wheel was taken where an sdist existed"
    );
}

/// The size cap, against a body a registry really serves: the refusal has to
/// come from the length the registry declares, before the body is read, or
/// the cap is a report on memory already spent.
#[tokio::test]
#[ignore = "networked: fetches from static.crates.io"]
async fn an_archive_over_the_cap_is_refused_before_it_is_downloaded() {
    let failure = Archive::live()
        .with_limit(1024)
        .fetch(Registry::Crates, "serde", "1.0.0")
        .await
        .expect_err("a kilobyte is smaller than any real crate");

    match failure {
        Failure::TooLarge { bytes, limit, .. } => {
            assert_eq!(limit, 1024);
            assert!(bytes > 1024, "got {bytes} bytes");
        }
        other => panic!("an archive over the cap is refused as too large, got {other:?}"),
    }
}

/// A version that does not exist is the commonest thing to get wrong, and the
/// answer has to be one a model can act on — ask for a version that exists —
/// rather than a status code. What the registry actually answers for a
/// missing archive is a fact about the registry, which is why this is here
/// and not in a suite that invents the response.
#[tokio::test]
#[ignore = "networked: fetches from registry.npmjs.org"]
async fn a_version_no_registry_has_is_a_failure_a_model_can_act_on() {
    let failure = Archive::live()
        .fetch(Registry::Npm, "zod", "99.99.99")
        .await
        .expect_err("npm has no such version of zod");

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
// Where a package's versions come from
// ---------------------------------------------------------------------------
//
// The fixture suite proves what this server does with a version document.
// These prove that the documents are the ones these three sources actually
// serve, and that each still carries a date per version — which is the field
// the whole order rests on, and the one a source is free to stop sending.
//
// None of them asserts a version number as the newest. That was checked by
// hand at implementation time and written into #18; asserting it here would
// be a test that fails the next time somebody publishes. What is asserted is
// what stays true: a release this server has seen is still listed, and the
// newest is recent.

/// npm, end to end. A scoped name is the case worth a real request: it is one
/// escaped path segment, and getting that wrong is a 404 for every `@types/*`
/// package there is.
#[tokio::test]
#[ignore = "networked: fetches from registry.npmjs.org"]
async fn npm_lists_the_versions_this_server_asks_it_for() {
    let versions = Catalogue::live()
        .versions(Registry::Npm, "@types/node")
        .await
        .expect("npm lists this package");

    assert!(
        versions.iter().any(|v| v.version == "20.1.0"),
        "a release this server has an archive fixture for is still published"
    );
    assert_recent(&versions, "2025");
}

/// crates.io, end to end, from the API host rather than the static one.
#[tokio::test]
#[ignore = "networked: fetches from crates.io"]
async fn crates_io_lists_the_versions_this_server_asks_it_for() {
    let versions = Catalogue::live()
        .versions(Registry::Crates, "serde")
        .await
        .expect("crates.io lists this crate");

    assert!(
        versions.iter().any(|v| v.version == "1.0.0"),
        "a release this server has an archive fixture for is still published"
    );
    assert_recent(&versions, "2025");
}

/// PyPI through deps.dev, and the one that would have shipped wrong.
///
/// #18 specified that this source lists oldest-first and should be reversed.
/// It does not: it sorts lexically by version string, so `requests` ends at
/// 2.9.2 and the reversal announces a 2016 release as the newest. The fixture
/// suite proves the ordering against a document that cannot change; this
/// proves the same thing against the source itself, which can.
#[tokio::test]
#[ignore = "networked: fetches from api.deps.dev"]
async fn pypi_versions_are_ordered_by_date_rather_than_by_the_sources_order() {
    let versions = Catalogue::live()
        .versions(Registry::PyPi, "requests")
        .await
        .expect("deps.dev lists this package");

    assert!(
        versions.iter().any(|v| v.version == "2.31.0"),
        "a release this server has an archive fixture for is still published"
    );

    let newest = versions.first().expect("a published package has versions");
    assert_ne!(
        newest.version, "2.9.2",
        "2.9.2 is the last entry in deps.dev's own order, so this is what \
         reversing the document produces"
    );
    assert_recent(&versions, "2024");
}

/// A package no registry has, asked for by name rather than by version.
#[tokio::test]
#[ignore = "networked: fetches from registry.npmjs.org"]
async fn a_package_no_registry_has_is_a_failure_a_model_can_act_on() {
    let failure = Catalogue::live()
        .versions(Registry::Npm, "diffpack-no-such-package-ever-published")
        .await
        .expect_err("npm has no such package");

    match failure {
        Failure::NoSuchPackage { package, .. } => {
            assert_eq!(package, "diffpack-no-such-package-ever-published");
        }
        other => panic!("a missing package should say so, got {other:?}"),
    }
}

/// The newest entry was published in or after `year`, and the list is in the
/// order this server promises.
///
/// The year is a floor rather than a value: a package that has had a release
/// since then is one whose source is still answering with real dates, and it
/// does not go stale the way a version number would.
fn assert_recent(versions: &[Version], year: &str) {
    let newest = versions.first().expect("a published package has versions");
    let published_at = newest
        .published_at
        .as_deref()
        .expect("the newest release is one the source dated");
    assert!(
        published_at >= year,
        "the newest release should not predate {year}, got {} at {published_at}",
        newest.version,
    );

    let dates: Vec<Option<&str>> = versions.iter().map(|v| v.published_at.as_deref()).collect();
    let mut sorted = dates.clone();
    sorted.sort_unstable();
    sorted.reverse();
    assert_eq!(
        dates, sorted,
        "the answer is newest first, with whatever the source left undated last"
    );
}
