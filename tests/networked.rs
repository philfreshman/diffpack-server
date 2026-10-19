use std::future::Future;
use std::sync::OnceLock;
use std::time::Instant;

use diffpack_server::archive::{Archive, At};
use diffpack_server::catalogue::Catalogue;
use diffpack_server::error::Failure;
use diffpack_server::registry::{Registry, Version, Versions};
use diffpack_server::search::Search;
use tokio::runtime::Runtime;

#[tokio::test]
#[ignore = "networked: fetches from registry.npmjs.org"]
async fn npm_serves_the_archive_this_server_asks_it_for() {
    let files = Archive::live()
        .fetch(Registry::Npm, "@types/node", "20.1.0")
        .await
        .expect("npm serves this version");

    assert!(
        files.at("package.json") != At::Nothing,
        "every npm package has a `package.json` at its root"
    );
}

#[tokio::test]
#[ignore = "networked: fetches from static.crates.io"]
async fn crates_io_serves_the_archive_this_server_asks_it_for() {
    let files = Archive::live()
        .fetch(Registry::Crates, "serde", "1.0.0")
        .await
        .expect("crates.io serves this version");

    assert!(
        files.at("Cargo.toml") != At::Nothing,
        "every crate carries a `Cargo.toml` at its root"
    );
}

#[tokio::test]
#[ignore = "networked: fetches from pypi.org and files.pythonhosted.org"]
async fn pypi_serves_the_source_distribution_it_lists() {
    let files = Archive::live()
        .fetch(Registry::PyPi, "requests", "2.31.0")
        .await
        .expect("PyPI lists and serves this version");

    assert!(
        files.at("setup.py") != At::Nothing,
        "the source distribution is the one to diff, and it carries `setup.py`"
    );
    assert!(
        !files
            .paths()
            .iter()
            .any(|path| path.contains(".dist-info/")),
        "`.dist-info/` means the wheel was taken where an sdist existed"
    );
}

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

#[tokio::test]
#[ignore = "networked: fetches from registry.npmjs.org"]
async fn npm_lists_the_versions_this_server_asks_it_for() {
    let versions = Catalogue::live()
        .versions(Registry::Npm, "@types/node")
        .await
        .expect("npm lists this package");

    assert!(
        versions.all.iter().any(|v| v.version == "20.1.0"),
        "a release this server has an archive fixture for is still published"
    );
    assert_recent(&versions.all, "2025");
    assert_points_somewhere(&versions, "`dist-tags.latest`");
}

#[tokio::test]
#[ignore = "networked: fetches from crates.io"]
async fn crates_io_lists_the_versions_this_server_asks_it_for() {
    let versions = Catalogue::live()
        .versions(Registry::Crates, "serde")
        .await
        .expect("crates.io lists this crate");

    assert!(
        versions.all.iter().any(|v| v.version == "1.0.0"),
        "a release this server has an archive fixture for is still published"
    );
    assert_recent(&versions.all, "2025");
    assert_points_somewhere(&versions, "`crate.default_version`");
}

#[tokio::test]
#[ignore = "networked: fetches from api.deps.dev"]
async fn pypi_versions_are_ordered_by_date_rather_than_by_the_sources_order() {
    let versions = Catalogue::live()
        .versions(Registry::PyPi, "requests")
        .await
        .expect("deps.dev lists this package");

    assert!(
        versions.all.iter().any(|v| v.version == "2.31.0"),
        "a release this server has an archive fixture for is still published"
    );

    let newest = versions
        .all
        .first()
        .expect("a published package has versions");
    assert_ne!(
        newest.version, "2.9.2",
        "2.9.2 is the last entry in deps.dev's own order, so this is what \
         reversing the document produces"
    );
    assert_recent(&versions.all, "2024");
    assert_points_somewhere(&versions, "an `isDefault` version");
}

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

fn assert_points_somewhere(versions: &Versions, field: &str) {
    let current = versions
        .current
        .as_deref()
        .unwrap_or_else(|| panic!("the registry names a current version in {field}"));

    assert!(
        versions.all.iter().any(|v| v.version == current),
        "{field} says {current} is current and the same document does not \
         list it, which on a package this ordinary means the field this \
         server reads has moved"
    );
}

#[tokio::test]
#[ignore = "networked: searches registry.npmjs.org"]
async fn npm_answers_a_search_in_the_shape_this_server_reads() {
    let hits = Search::live()
        .hits(Registry::Npm, "zod", 5)
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

#[tokio::test]
#[ignore = "networked: searches crates.io"]
async fn crates_io_answers_a_search_in_the_shape_this_server_reads() {
    let hits = Search::live()
        .hits(Registry::Crates, "serde", 5)
        .await
        .expect("crates.io answers a search");

    let first = hits.first().expect("`serde` is a crate crates.io has");
    assert_eq!(first.name, "serde", "got {hits:?}");
    assert!(
        first.version.is_some(),
        "crates.io says which version it would install, got {first:?}"
    );
}

#[tokio::test]
#[ignore = "networked: fetches the whole of pypi.org/simple/"]
async fn pypi_answers_a_search_out_of_the_index_it_publishes() {
    let search = Search::live();

    let hits = search
        .hits(Registry::PyPi, "pyyaml", 5)
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

#[tokio::test]
#[ignore = "networked: fetches the whole of pypi.org/simple/"]
async fn a_second_pypi_search_is_answered_from_the_index_already_fetched() {
    let search = Search::live();

    search
        .hits(Registry::PyPi, "requests", 5)
        .await
        .expect("PyPI serves its index");

    let started = Instant::now();
    let hits = search
        .hits(Registry::PyPi, "flask", 5)
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

#[test]
#[ignore = "networked: fetches from proxy.golang.org"]
fn the_go_proxy_serves_a_module_with_capitals_under_its_escaped_path() {
    on_one_runtime(async {
        let files = Archive::live()
            .fetch(Registry::Go, "github.com/Masterminds/semver/v3", "v3.2.1")
            .await
            .expect("the proxy serves this version");

        assert!(
            files.at("go.mod") != At::Nothing,
            "every module zip carries its `go.mod`, at the module's own root"
        );
        assert!(
            !files.paths().iter().any(|path| path.contains("@v3.2.1")),
            "the `{{module}}@{{version}}/` prefix is gone from every path"
        );
    });
}

#[test]
#[ignore = "networked: fetches from proxy.golang.org"]
fn two_versions_of_a_go_module_share_their_paths() {
    on_one_runtime(async {
        let archive = Archive::live();
        let module = "github.com/go-chi/chi/v5";
        let older = archive
            .fetch(Registry::Go, module, "v5.0.11")
            .await
            .expect("the proxy serves v5.0.11");
        let newer = archive
            .fetch(Registry::Go, module, "v5.0.12")
            .await
            .expect("the proxy serves v5.0.12");

        for path in ["go.mod", "chi.go", "mux.go"] {
            assert!(
                older.at(path) != At::Nothing && newer.at(path) != At::Nothing,
                "`{path}` is at one path in both versions, so a diff compares it \
                 rather than reading it as removed and re-added"
            );
        }
    });
}

#[test]
#[ignore = "networked: fetches from proxy.golang.org"]
fn the_go_proxy_lists_a_major_version_modules_versions_highest_first() {
    on_one_runtime(async {
        let versions = Catalogue::live()
            .versions(Registry::Go, "github.com/go-chi/chi/v5")
            .await
            .expect("the proxy lists this module");

        let listed: Vec<&str> = versions.all.iter().map(|v| v.version.as_str()).collect();
        assert!(
            listed.iter().all(|version| version.starts_with("v5.")),
            "`/v5` is the module path's, so every version it lists is a v5: got {listed:?}"
        );

        let position = |version: &str| {
            listed
                .iter()
                .position(|listed| *listed == version)
                .unwrap_or_else(|| panic!("{version} is published, got {listed:?}"))
        };
        assert!(
            position("v5.0.12") < position("v5.0.11") && position("v5.0.11") < position("v5.0.2"),
            "highest first, with 12 above 2 rather than below it: got {listed:?}"
        );
        assert_points_somewhere(&versions, "`@latest`");
    });
}

#[test]
#[ignore = "networked: fetches from proxy.golang.org"]
fn an_untagged_go_module_is_listed_as_the_pseudo_version_latest_resolves() {
    on_one_runtime(async {
        let versions = Catalogue::live()
            .versions(Registry::Go, "golang.org/x/exp")
            .await
            .expect("the proxy resolves this module");

        let [only] = versions.all.as_slice() else {
            panic!(
                "a module with no tags is the one version `@latest` resolves, got {:?}",
                versions.all
            );
        };
        assert!(
            only.version.starts_with("v0.0.0-"),
            "an untagged module resolves to a pseudo-version, got {}",
            only.version
        );
        assert_eq!(versions.current.as_deref(), Some(only.version.as_str()));
    });
}

#[test]
#[ignore = "networked: fetches from proxy.golang.org"]
fn the_go_proxy_answers_a_whole_module_path_and_nothing_less() {
    on_one_runtime(async {
        let search = Search::live();

        let hits = search
            .hits(Registry::Go, "github.com/go-chi/chi/v5", 5)
            .await
            .expect("the proxy answers for a module it has");
        let [hit] = hits.as_slice() else {
            panic!("a module path is one module, got {hits:?}");
        };
        assert_eq!(hit.name, "github.com/go-chi/chi/v5");
        assert!(
            hit.version.as_deref().is_some_and(|v| v.starts_with("v5.")),
            "got {hit:?}"
        );

        let none = search
            .hits(Registry::Go, "chi", 5)
            .await
            .expect("a name that is not a module path is a miss, not a failure");
        assert!(none.is_empty(), "got {none:?}");
    });
}

fn on_one_runtime(test: impl Future<Output = ()>) {
    static RUNTIME: OnceLock<Runtime> = OnceLock::new();

    RUNTIME
        .get_or_init(|| Runtime::new().expect("a test can start a runtime"))
        .block_on(test);
}
