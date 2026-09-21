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
