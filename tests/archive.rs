use diffpack_server::archive::{self, Archive, At, FileMap};
use diffpack_server::error::Failure;
use diffpack_server::registry::Registry;

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

#[tokio::test]
async fn a_packagist_version_arrives_as_the_files_of_the_archive_its_listing_names() {
    let files = fixtures()
        .fetch(Registry::Composer, "monolog/monolog", "3.12.1")
        .await
        .expect("the listing names a GitHub zipball, fetched from codeload");

    assert_eq!(
        paths(&files),
        vec![
            "README.md",
            "composer.json",
            "src",
            "src/Monolog",
            "src/Monolog/Logger.php"
        ],
        "codeload's `Seldaek-monolog-d7059e4/` folder is not part of the version's files"
    );
    assert!(
        text(&files, "src/Monolog/Logger.php").contains("'3.12.1'"),
        "the archive is the one listed for the version asked for"
    );
}

#[tokio::test]
async fn a_version_packagist_does_not_list_is_no_such_version_with_the_ones_it_does() {
    let failure = fixtures()
        .fetch(Registry::Composer, "monolog/monolog", "9.9.9")
        .await
        .expect_err("9.9.9 is not in the listing");

    match failure {
        Failure::NoSuchVersion {
            registry,
            version,
            known,
            ..
        } => {
            assert_eq!(registry, "Packagist");
            assert_eq!(version, "9.9.9");
            assert_eq!(
                known.first().map(String::as_str),
                Some("3.12.1"),
                "the listing is in hand, so the refusal can say what is there"
            );
        }
        other => panic!("a version the listing lacks is no_such_version, got {other:?}"),
    }
}

#[tokio::test]
async fn a_packagist_version_without_an_archive_this_server_reads_is_refused_as_such() {
    for (version, why) in [
        ("1.0.0", "its dist is unset"),
        (
            "2.0.0",
            "its dist is on a host this server does not fetch from",
        ),
    ] {
        let failure = fixtures()
            .fetch(Registry::Composer, "diffpack/awkward", version)
            .await
            .expect_err(why);

        match failure {
            Failure::MalformedArchive { reason, .. } => assert!(
                reason.contains("lists no archive"),
                "{version}: the refusal should say why, got {reason}"
            ),
            other => panic!("{version} is listed but has no archive here, got {other:?}"),
        }
    }
}

#[test]
fn the_size_cap_is_above_what_a_large_package_weighs() {
    let a_large_crate: u64 = 80_000_000;

    assert!(
        archive::SIZE_LIMIT > a_large_crate,
        "an 80 MB crate is an ordinary thing to diff, got a cap of {} bytes",
        archive::SIZE_LIMIT
    );
}

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

fn fixtures() -> Archive {
    Archive::fixture(concat!(env!("CARGO_MANIFEST_DIR"), "/fixtures/archives"))
}

fn text<'a>(files: &'a FileMap, path: &str) -> &'a str {
    match files.at(path) {
        At::File(file) => file.text(),
        other => panic!(
            "`{path}` should be a file in the file map, got {other:?} among {:?}",
            paths(files)
        ),
    }
}

fn has(files: &FileMap, path: &str) -> bool {
    files.at(path) != At::Nothing
}

fn paths(files: &FileMap) -> Vec<&str> {
    files.paths()
}
