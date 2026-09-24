//! A version's files, asked of the module that fetches them.
//!
//! The seam is [`Archive::fetch`] and nothing below it: a caller asks for a
//! package's files and is given a [`FileMap`] or a
//! [`Failure`](diffpack_server::error::Failure). Which URL that took, how
//! many requests it was, and which library untarred the bytes are this
//! module's business, so no test here names any of them.
//!
//! Every test below drives the fixture adapter, which reads
//! `fixtures/archives/` instead of a registry — so the suite is offline and
//! deterministic. The adapter is keyed by *URL*, which is what makes these
//! tests say something about resolution too: a fetch path that built a
//! different URL than `resolve_archive_url` answers with would miss the
//! fixture and fail here, rather than the two quietly disagreeing.
//!
//! The live adapter is exercised in `tests/networked.rs`, which reaches the
//! real registries and is `#[ignore]`d for that reason.

use diffpack_server::archive::{self, Archive, At, FileMap};
use diffpack_server::error::Failure;
use diffpack_server::registry::Registry;

/// The npm rule that is not obvious, through the whole seam: the path keeps a
/// scope and the filename drops it, so the bytes for `@types/node` come back
/// from `node-20.1.0.tgz`. A fetch path that spelled that URL itself rather
/// than asking `registry` would find no fixture here.
///
/// The expected content is the fixture's, which was written by hand and
/// packed by `tar` — not produced by this crate.
#[tokio::test]
async fn an_npm_tarball_arrives_as_the_files_inside_it() {
    let files = fixtures()
        .fetch(Registry::Npm, "@types/node", "20.1.0")
        .await
        .expect("the fixture adapter has the archive npm serves for this version");

    assert_eq!(
        text(&files, "package.json"),
        "{\n  \"name\": \"@types/node\",\n  \"version\": \"20.1.0\"\n}\n",
        "a file in the archive should arrive with its content"
    );
    assert!(
        has(&files, "index.d.ts"),
        "every file in the archive should be in the map, got {:?}",
        paths(&files)
    );
}

/// npm wraps a package in `package/` and crates.io in `{name}-{version}/`,
/// and CONTEXT.md says a FileMap has that directory already stripped: a
/// caller reads `package.json`, not `package/package.json`. It is the
/// difference between two registries' archives being comparable and not.
#[tokio::test]
async fn the_archives_top_level_directory_is_already_stripped() {
    let files = fixtures()
        .fetch(Registry::Npm, "@types/node", "20.1.0")
        .await
        .expect("the fixture adapter has this archive");

    assert!(
        !paths(&files)
            .iter()
            .any(|path| path.starts_with("package/")),
        "the wrapper directory is not part of a version's files, got {:?}",
        paths(&files)
    );
}

/// PyPI is the registry whose archive is not at a path anyone can build: a
/// version's files are listed in its metadata and chosen from. Both hops are
/// behind the seam, so a caller asks the same question it asks of npm.
///
/// A version with both an sdist and a wheel resolves to the sdist. A wheel is
/// built output, so a diff of one is not the diff anybody asked for — and the
/// choice is observable here rather than by inspecting a URL, because only
/// the sdist fixture carries `setup.py`.
#[tokio::test]
async fn a_pypi_version_with_a_source_distribution_arrives_as_the_source() {
    let files = fixtures()
        .fetch(Registry::PyPi, "requests", "2.31.0")
        .await
        .expect("the fixture adapter has this version's metadata and its sdist");

    assert!(
        has(&files, "setup.py"),
        "a source distribution carries `setup.py`, and a wheel does not: got {:?}",
        paths(&files)
    );
    assert_eq!(
        text(&files, "requests/__init__.py"),
        "__version__ = \"2.31.0\"\n",
        "the sdist's files are the version's files"
    );
    assert!(
        !paths(&files)
            .iter()
            .any(|path| path.contains(".dist-info/")),
        "`.dist-info/` is a wheel's, so its presence means the wheel was chosen: got {:?}",
        paths(&files)
    );
}

/// Some projects publish no source distribution at all. A wheel is then the
/// only thing there is to diff, so it is what a caller gets — refusing would
/// make a whole registry's largest packages unanswerable.
///
/// A wheel is a zip with two top-level directories, so nothing is stripped
/// from its paths: the whole of the difference between this map and an
/// sdist's is visible here.
#[tokio::test]
async fn a_pypi_version_with_only_a_wheel_arrives_as_the_wheel() {
    let files = fixtures()
        .fetch(Registry::PyPi, "tensorflow", "2.16.1")
        .await
        .expect("a version with only a wheel still has files to read");

    assert_eq!(
        text(&files, "tensorflow/__init__.py"),
        "VERSION = \"2.16.1\"\n",
        "the wheel's files are the version's files"
    );
    assert!(
        has(&files, "tensorflow-2.16.1.dist-info/METADATA"),
        "a wheel's two top-level directories are both kept, got {:?}",
        paths(&files)
    );
}

/// crates.io serves a `.crate`, which is a gzip'd tar wrapping the package in
/// `{name}-{version}/`. A different extension and a different wrapper than
/// npm's, and the same map out the other side — which is the point of the
/// seam: what a caller reads does not vary by registry.
#[tokio::test]
async fn a_crates_io_crate_arrives_as_the_files_inside_it() {
    let files = fixtures()
        .fetch(Registry::Crates, "serde", "1.0.0")
        .await
        .expect("the fixture adapter has the archive crates.io serves");

    assert_eq!(
        text(&files, "src/lib.rs"),
        "pub fn serialize() {}\n",
        "a crate's files arrive under the paths inside it, one level up"
    );
    assert!(has(&files, "Cargo.toml"), "got {:?}", paths(&files));
}

/// The fifth shape a registry serves: a zip source distribution, which PyPI
/// still has for older releases — numpy shipped one for years. It is neither
/// gzip'd nor a tar, so it is the case that would break an extractor that
/// assumed either.
#[tokio::test]
async fn a_pypi_zip_source_distribution_arrives_as_its_files() {
    let files = fixtures()
        .fetch(Registry::PyPi, "numpy", "1.9.0")
        .await
        .expect("a zip sdist is an archive this server can read");

    assert_eq!(
        text(&files, "numpy/__init__.py"),
        "__version__ = \"1.9.0\"\n",
        "a zip's entries are files in the map like any other archive's"
    );
    assert!(has(&files, "setup.py"), "got {:?}", paths(&files));
}

// ---------------------------------------------------------------------------
// What this server will not download
// ---------------------------------------------------------------------------

/// The cap production runs with has to sit above what an ordinary package
/// weighs — #10 puts an 80 MB crate in the normal range and a 5 GB one in the
/// attack range. A cap below the first would refuse packages people diff
/// every day, which is a worse failure than the one it is guarding against.
#[test]
fn the_size_cap_is_above_what_a_large_package_weighs() {
    let a_large_crate: u64 = 80_000_000;

    assert!(
        archive::SIZE_LIMIT > a_large_crate,
        "an 80 MB crate is an ordinary thing to diff, got a cap of {} bytes",
        archive::SIZE_LIMIT
    );
}

/// A package name in a tool argument must never become a request to a host
/// of somebody else's choosing. The one place a URL this server did not build
/// can enter is a listing: PyPI names its own download hosts, and a registry
/// that named another one would be handing this server an outbound request.
///
/// The fixture set *has* an archive at that off-allowlist URL, so this test
/// fails if the guard is missing rather than passing because the fixture was
/// absent: without the check the fetch would succeed and return files.
#[tokio::test]
async fn a_listing_pointing_at_a_host_outside_the_allowlist_is_not_fetched() {
    let failure = fixtures()
        .fetch(Registry::PyPi, "sneaky", "1.0.0")
        .await
        .expect_err("a download at an unexpected host is not one this server makes");

    match failure {
        Failure::MalformedArchive { reason, .. } => assert!(
            reason.contains("host"),
            "the refusal should say what it objected to, got {reason}"
        ),
        other => panic!("the archive should be refused, got {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// Driving the seam
// ---------------------------------------------------------------------------

/// The adapter that reads `fixtures/archives/` rather than a registry.
fn fixtures() -> Archive {
    Archive::fixture(concat!(env!("CARGO_MANIFEST_DIR"), "/fixtures/archives"))
}

/// The text of the file at `path`, or a panic naming what the map holds.
fn text<'a>(files: &'a FileMap, path: &str) -> &'a str {
    match files.at(path) {
        At::File(file) => file.text(),
        other => panic!(
            "`{path}` should be a file in the file map, got {other:?} among {:?}",
            paths(files)
        ),
    }
}

/// Whether the map has anything at `path`, a file or a directory.
fn has(files: &FileMap, path: &str) -> bool {
    files.at(path) != At::Nothing
}

/// Every path in the map, in order, for a failure message worth reading.
fn paths(files: &FileMap) -> Vec<&str> {
    files.paths()
}
