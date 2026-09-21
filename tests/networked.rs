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

use std::time::Instant;

use diffpack_server::archive::Archive;
use diffpack_server::catalogue::Catalogue;
use diffpack_server::error::Failure;
use diffpack_server::registry::Registry;

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
// What a registry publishes
// ---------------------------------------------------------------------------

/// npm's search endpoint, which is the easy one: it takes a query and answers
/// it. What only a real request settles is that the shape this server reads
/// is the shape npm sends.
#[tokio::test]
#[ignore = "networked: searches registry.npmjs.org"]
async fn npm_answers_a_search_in_the_shape_this_server_reads() {
    let hits = Catalogue::live()
        .search(Registry::Npm, "zod", 5)
        .await
        .expect("npm answers a search");

    let first = hits.first().expect("`zod` is a package npm has");
    assert_eq!(first.name, "zod", "got {hits:?}");
    assert!(
        first.version.is_some() && first.description.is_some(),
        "npm carries all three fields, got {first:?}"
    );
    assert!(hits.len() <= 5, "five were asked for, got {}", hits.len());
}

/// crates.io, the same question asked of a different shape.
#[tokio::test]
#[ignore = "networked: searches crates.io"]
async fn crates_io_answers_a_search_in_the_shape_this_server_reads() {
    let hits = Catalogue::live()
        .search(Registry::Crates, "serde", 5)
        .await
        .expect("crates.io answers a search");

    let first = hits.first().expect("`serde` is a crate crates.io has");
    assert_eq!(first.name, "serde", "got {hits:?}");
    assert!(
        first.version.is_some(),
        "crates.io says which version it would install, got {first:?}"
    );
}

/// PyPI, which is the whole index and the reason #19 needed deciding. Three
/// things only a real request can settle: that the index still answers JSON
/// when asked for PEP 691's media type, that it arrives inside the upstream
/// timeout at all — it is 44 MB uncompressed — and that a name spelled the
/// way PyPI spells it comes back spelled that way.
#[tokio::test]
#[ignore = "networked: fetches the whole of pypi.org/simple/"]
async fn pypi_answers_a_search_out_of_the_index_it_publishes() {
    let catalogue = Catalogue::live();

    let hits = catalogue
        .search(Registry::PyPi, "pyyaml", 5)
        .await
        .expect("PyPI serves its index");

    let first = hits.first().expect("`PyYAML` is a project PyPI has");
    assert_eq!(
        first.name, "PyYAML",
        "the index's own spelling, which is what the other tools take, got {hits:?}"
    );
    assert_eq!(
        (first.version.as_ref(), first.description.as_ref()),
        (None, None),
        "the index carries neither, and a hit does not invent them, got {first:?}"
    );
}

/// And the second search does not fetch it again. A 44 MB document per query
/// is a tool an agent stops using, so the index a warm instance already has
/// is the one it answers from until it goes stale.
///
/// Timed rather than counted, because what is being asserted is the cost: the
/// first search is seconds of transfer and the second is a scan of a string
/// already in memory.
#[tokio::test]
#[ignore = "networked: fetches the whole of pypi.org/simple/"]
async fn a_second_pypi_search_is_answered_from_the_index_already_fetched() {
    let catalogue = Catalogue::live();

    catalogue
        .search(Registry::PyPi, "requests", 5)
        .await
        .expect("PyPI serves its index");

    let started = Instant::now();
    let hits = catalogue
        .search(Registry::PyPi, "flask", 5)
        .await
        .expect("and the second search is answered from it");
    let spent = started.elapsed();

    assert!(
        hits.iter().any(|hit| hit.name == "Flask"),
        "a different query is matched against the same index, got {hits:?}"
    );
    assert!(
        spent < std::time::Duration::from_secs(2),
        "a search answered from the index already held should cost a scan \
         rather than a download, took {spent:?}"
    );
}
