use diffpack_server::error::Failure;
use diffpack_server::registry::{self, ArchiveSource, Hit, Registry, SearchSource, VersionSource};

#[test]
fn every_registry_carries_the_identifier_and_the_name_it_is_known_by() {
    let known: Vec<(&str, &str)> = Registry::ALL
        .iter()
        .map(|registry| (registry.id(), registry.name()))
        .collect();

    assert_eq!(
        known,
        vec![
            ("npm", "npm"),
            ("crates", "crates.io"),
            ("pypi", "PyPI"),
            ("go", "Go"),
            ("composer", "Packagist")
        ],
        "the five registries of CONTEXT.md, in the order a catalogue lists them"
    );
}

#[test]
fn a_registry_parses_from_its_identifier_and_nothing_else() {
    for registry in Registry::ALL {
        assert_eq!(
            Registry::from_id(registry.id()),
            Some(registry),
            "`{}` should parse back to the registry it names",
            registry.id()
        );
    }

    assert_eq!(
        Registry::from_id("crates.io"),
        None,
        "the name a model reads is not the identifier the wire sends"
    );
    assert_eq!(
        Registry::from_id("NPM"),
        None,
        "nothing here is normalised, case included"
    );
    assert_eq!(
        Registry::from_id("golang"),
        None,
        "Go's identifier is `go`, the one the cache key and the web app use"
    );
}

#[test]
fn a_registry_serialises_as_the_identifier_it_parses_from() {
    for registry in Registry::ALL {
        let json = serde_json::to_value(registry).expect("a registry serialises");

        assert_eq!(
            json,
            serde_json::json!(registry.id()),
            "a registry on the wire is its identifier and nothing else"
        );
        assert_eq!(
            serde_json::from_value::<Registry>(json).expect("and reads back"),
            registry
        );
    }
}

#[test]
fn an_unknown_identifier_is_refused_by_naming_the_ones_that_exist() {
    let refusal = serde_json::from_value::<Registry>(serde_json::json!("maven"))
        .expect_err("Maven is not a registry this server knows")
        .to_string();

    for known in Registry::ALL {
        assert!(
            refusal.contains(known.id()),
            "the refusal should name `{}`, got {refusal}",
            known.id()
        );
    }
}

#[test]
fn the_schema_lists_exactly_the_registries_this_server_has() {
    let schema = serde_json::to_value(schemars::schema_for!(Registry)).expect("a schema is JSON");

    assert_eq!(
        schema["type"], "string",
        "a registry is a string on the wire, got {schema}"
    );
    assert_eq!(
        schema["enum"],
        serde_json::json!(["npm", "crates", "pypi", "go", "composer"]),
        "the schema's values are the identifiers, in the catalogue's order"
    );
}

#[test]
fn every_registry_says_how_to_reach_a_version_archive() {
    let source = |registry: Registry, package: &str, version: &str| {
        registry
            .archive(package, version)
            .expect("a registry this server has can say where its archives are")
    };

    assert_eq!(
        source(Registry::Npm, "zod", "4.0.0"),
        ArchiveSource::Archive {
            url: "https://registry.npmjs.org/zod/-/zod-4.0.0.tgz".to_owned()
        }
    );
    assert_eq!(
        source(Registry::Npm, "@types/node", "20.1.0"),
        ArchiveSource::Archive {
            url: "https://registry.npmjs.org/@types/node/-/node-20.1.0.tgz".to_owned()
        },
        "a scoped name keeps the scope in the path and drops it from the filename"
    );
    assert_eq!(
        source(Registry::Crates, "serde", "1.0.0"),
        ArchiveSource::Archive {
            url: "https://static.crates.io/crates/serde/serde-1.0.0.crate".to_owned()
        },
        "crates.io serves archives from the static host, not the API one"
    );
    assert_eq!(
        source(Registry::PyPi, "requests", "2.31.0"),
        ArchiveSource::Listing {
            url: "https://pypi.org/pypi/requests/2.31.0/json".to_owned()
        },
        "PyPI's archive URL is in the version's metadata, so that is the first hop"
    );
    assert_eq!(
        source(Registry::Go, "github.com/Masterminds/semver/v3", "v3.2.1"),
        ArchiveSource::Archive {
            url: "https://proxy.golang.org/github.com/!masterminds/semver/v3/@v/v3.2.1.zip"
                .to_owned()
        },
        "the proxy serves a module path lower-cased, each capital marked with `!`"
    );
    assert_eq!(
        source(Registry::Go, "github.com/go-chi/chi/v5", "v5.0.12"),
        ArchiveSource::Archive {
            url: "https://proxy.golang.org/github.com/go-chi/chi/v5/@v/v5.0.12.zip".to_owned()
        },
        "a major-version suffix is part of the module path, not a version"
    );
    assert_eq!(
        source(Registry::Composer, "monolog/monolog", "3.12.1"),
        ArchiveSource::Listing {
            url: "https://repo.packagist.org/p2/monolog/monolog.json".to_owned()
        },
        "a Packagist listing names every version's archive, so that is the first hop"
    );
}

#[test]
fn a_name_packagist_could_not_list_is_no_such_package() {
    for name in ["monolog", "monolog/", "-monolog/monolog", "a/b/c"] {
        assert!(
            matches!(
                Registry::Composer.archive(name, "1.0.0"),
                Err(Failure::NoSuchPackage { ref registry, ref package })
                    if registry == "Packagist" && package == name
            ),
            "`{name}` cannot be a Composer package, got {:?}",
            Registry::Composer.archive(name, "1.0.0")
        );
    }
}

#[test]
fn a_listing_is_read_for_the_archive_a_diff_wants() {
    let listing = |files: &str| format!(r#"{{"urls":[{files}]}}"#);
    let sdist = r#"{"packagetype":"sdist","url":"https://files.pythonhosted.org/x-1.0.tar.gz"}"#;
    let wheel = r#"{"packagetype":"bdist_wheel","url":"https://files.pythonhosted.org/x-py3-none-any.whl"}"#;

    assert_eq!(
        Registry::PyPi.choose_archive("1.0", &listing(&format!("{wheel},{sdist}"))),
        Some("https://files.pythonhosted.org/x-1.0.tar.gz".to_owned()),
        "an sdist is preferred over a wheel, whichever order they are listed in"
    );
    assert_eq!(
        Registry::PyPi.choose_archive("9.9", &listing(&format!("{wheel},{sdist}"))),
        Some("https://files.pythonhosted.org/x-1.0.tar.gz".to_owned()),
        "a PyPI listing is already for one version, so the version asked for does not change the choice"
    );
    assert_eq!(
        Registry::PyPi.choose_archive("1.0", &listing(wheel)),
        Some("https://files.pythonhosted.org/x-py3-none-any.whl".to_owned()),
        "a version published only as a wheel is still diffable"
    );
    assert_eq!(
        Registry::PyPi.choose_archive("1.0", &listing("")),
        None,
        "a version with no file this server can read names no archive"
    );
    assert_eq!(
        Registry::PyPi.choose_archive("1.0", "not metadata"),
        None,
        "and neither does a body that is not the metadata we asked for"
    );
}

#[test]
fn every_registry_says_where_its_versions_are_listed() {
    assert_eq!(
        Registry::Npm.versions("zod"),
        VersionSource {
            url: "https://registry.npmjs.org/zod".to_owned(),
            current: None,
        }
    );
    assert_eq!(
        Registry::Npm.versions("@types/node").url,
        "https://registry.npmjs.org/%40types%2Fnode",
        "a scoped name is one package name, so it is one escaped path segment"
    );
    assert_eq!(
        Registry::Crates.versions("serde"),
        VersionSource {
            url: "https://crates.io/api/v1/crates/serde".to_owned(),
            current: None,
        }
    );
    assert_eq!(
        Registry::PyPi.versions("requests"),
        VersionSource {
            url: "https://api.deps.dev/v3/systems/pypi/packages/requests".to_owned(),
            current: None,
        },
        "PyPI's own index is not a version list a client can read, so deps.dev is the source"
    );
    assert_eq!(
        Registry::Go.versions("github.com/Masterminds/semver/v3"),
        VersionSource {
            url: "https://proxy.golang.org/github.com/!masterminds/semver/v3/@v/list".to_owned(),
            current: Some(
                "https://proxy.golang.org/github.com/!masterminds/semver/v3/@latest".to_owned()
            ),
        },
        "the proxy's version list names versions and nothing else, so the version \
         `go get` installs is asked of `@latest` beside it"
    );
    assert_eq!(
        Registry::Go.versions("example.com/a module?#").url,
        "https://proxy.golang.org/example.com/a%20module%3F%23/@v/list",
        "a module path keeps its slashes, and anything that would end the path is escaped"
    );
    assert_eq!(
        Registry::Composer.versions("monolog/monolog"),
        VersionSource {
            url: "https://repo.packagist.org/p2/monolog/monolog.json".to_owned(),
            current: None,
        },
        "a Composer name is a vendor and a package, and each is one path segment"
    );
    assert_eq!(
        Registry::Composer.versions("vendor/a b?#").url,
        "https://repo.packagist.org/p2/vendor/a%20b%3F%23.json",
        "anything in a segment that would end the path is escaped"
    );
    assert_eq!(
        Registry::Composer.versions("a/b/c").url,
        "https://repo.packagist.org/p2/a/b%2Fc.json",
        "only the first slash separates the vendor from the package"
    );
}

#[test]
fn a_go_list_is_in_the_order_diffpack_io_shows_it() {
    let list = "v1.9.0\nv1.10.0\nv1.10.0-rc.1\nv1.10.0-rc.2\n\
                v0.0.0-20240101000000-abcdefabcdef\nv2.0.0+incompatible\nv1.10.1\n";

    let listed: Vec<String> = Registry::Go
        .read_versions(list)
        .expect("the proxy's list reads")
        .all
        .into_iter()
        .map(|version| version.version)
        .collect();

    assert_eq!(
        listed,
        vec![
            "v2.0.0+incompatible",
            "v1.10.1",
            "v1.10.0",
            "v1.10.0-rc.2",
            "v1.10.0-rc.1",
            "v1.9.0",
            "v0.0.0-20240101000000-abcdefabcdef",
        ],
        "the order diffpack.io's comparator gives the same list: 10 above 9, a \
         release above its own previews, `+incompatible` ignored"
    );
}

#[test]
fn a_go_list_with_a_line_that_is_not_a_version_does_not_read() {
    assert_eq!(
        Registry::Go.read_versions("v1.0.0\n<html>not found</html>\n"),
        None,
        "a Go version starts with `v`, so anything else is not the list that was asked for"
    );
}

const MINIFIED_LISTING: &str = r#"{"minified":"composer/2.0","packages":{"vendor/package":[
    {"name":"vendor/package","version":"2.0.0-RC1","version_normalized":"2.0.0.0-RC1",
     "time":"2026-03-01T00:00:00+00:00","dist":{"url":"https://api.github.com/repos/vendor/package/zipball/c"}},
    {"version":"1.1.0","version_normalized":"1.1.0.0",
     "time":"2026-02-01T00:00:00+00:00","dist":{"url":"https://api.github.com/repos/vendor/package/zipball/b"}},
    {"version":"1.0.1","version_normalized":"1.0.1.0"},
    {"version":"1.0.0","version_normalized":"1.0.0.0","time":"__unset",
     "dist":{"url":"https://api.github.com/repos/vendor/package/zipball/a"}},
    {"version":"0.9.0","version_normalized":"0.9.0.0","time":"2026-01-01T00:00:00+00:00"}
]}}"#;

fn listed(versions: &registry::Versions) -> Vec<(&str, Option<&str>)> {
    versions
        .all
        .iter()
        .map(|version| (version.version.as_str(), version.published_at.as_deref()))
        .collect()
}

#[test]
fn a_packagist_listing_carries_each_key_forward_until_it_changes_or_is_unset() {
    let versions = Registry::Composer
        .read_versions(MINIFIED_LISTING)
        .expect("a minified Packagist listing reads");

    assert_eq!(
        listed(&versions),
        vec![
            ("2.0.0-RC1", Some("2026-03-01T00:00:00+00:00")),
            ("1.1.0", Some("2026-02-01T00:00:00+00:00")),
            ("1.0.1", Some("2026-02-01T00:00:00+00:00")),
            ("0.9.0", Some("2026-01-01T00:00:00+00:00")),
            ("1.0.0", None),
        ],
        "1.0.1 inherits the date listed before it, 1.0.0's is unset so it goes last, \
         and 0.9.0 dates itself again"
    );
}

#[test]
fn packagists_current_version_is_the_first_stable_one_it_lists() {
    let versions = Registry::Composer
        .read_versions(MINIFIED_LISTING)
        .expect("a minified Packagist listing reads");

    assert_eq!(
        versions.current.as_deref(),
        Some("1.1.0"),
        "Composer has no pointer of its own, so it is the newest stable entry in \
         Packagist's order"
    );
    assert_eq!(
        versions
            .all
            .iter()
            .map(|version| (version.version.as_str(), version.prerelease))
            .collect::<Vec<_>>(),
        vec![
            ("2.0.0-RC1", true),
            ("1.1.0", false),
            ("1.0.1", false),
            ("0.9.0", false),
            ("1.0.0", false),
        ]
    );
}

#[test]
fn a_full_packagist_listing_carries_nothing_forward() {
    let listing = r#"{"packages":{"vendor/package":[
        {"version":"1.1.0","version_normalized":"1.1.0.0","time":"2026-02-01T00:00:00+00:00"},
        {"version":"1.0.0","version_normalized":"1.0.0.0"}
    ]}}"#;

    let versions = Registry::Composer
        .read_versions(listing)
        .expect("a listing that is not minified reads too");

    assert_eq!(
        listed(&versions),
        vec![
            ("1.1.0", Some("2026-02-01T00:00:00+00:00")),
            ("1.0.0", None),
        ],
        "without `minified`, an entry missing a date has none"
    );
}

#[test]
fn a_body_that_is_not_a_packagist_listing_does_not_read() {
    assert_eq!(
        Registry::Composer.read_versions("<html>not found</html>"),
        None
    );
    assert_eq!(
        Registry::Composer.read_versions(r#"{"packages":{}}"#),
        None,
        "a listing that names no package is not the one that was asked for"
    );
}

#[test]
fn a_composer_preview_is_its_stability_suffix() {
    for (version, preview) in [
        ("2.0.0.0-RC1", true),
        ("2.0.0.0-beta2", true),
        ("1.0.0.0-alpha", true),
        ("1.0.0.0-dev", true),
        ("1.0.x-dev", true),
        ("dev-main", true),
        ("1.0.0-a1", true),
        ("1.0.0-b2", true),
        ("1.0.0rc1", true),
        ("v1.0.0-beta.2", true),
        ("1.0.0.0", false),
        ("1.0.0.0-patch1", false),
        ("1.0.0-pl2", false),
        ("1.0.0-p1", false),
    ] {
        assert_eq!(
            Registry::Composer.is_prerelease(version),
            preview,
            "`{version}`"
        );
    }
}

#[test]
fn search_comes_from_the_registrys_own_index() {
    assert_eq!(
        Registry::Npm.search("zod", 10),
        SearchSource {
            url: "https://registry.npmjs.org/-/v1/search?text=zod&size=10".to_owned(),
            accept: "application/json",
            whole_index: false,
            names_a_package: false,
        }
    );
    assert_eq!(
        Registry::Crates.search("json parser", 5),
        SearchSource {
            url: "https://crates.io/api/v1/crates?q=json%20parser&per_page=5".to_owned(),
            accept: "application/json",
            whole_index: false,
            names_a_package: false,
        },
        "a query is a parameter value, so a space in it is escaped and not sent"
    );
    assert_eq!(
        Registry::PyPi.search("requests", 10),
        SearchSource {
            url: "https://pypi.org/simple/".to_owned(),
            accept: "application/vnd.pypi.simple.v1+json",
            whole_index: true,
            names_a_package: false,
        },
        "PyPI has no search endpoint, so the source is the index itself"
    );
    assert_eq!(
        Registry::Go.search("github.com/Masterminds/semver/v3", 10),
        SearchSource {
            url: "https://proxy.golang.org/github.com/!masterminds/semver/v3/@latest".to_owned(),
            accept: "application/json",
            whole_index: false,
            names_a_package: true,
        },
        "the Go proxy has no search either, so the query is asked for as the module \
         path it would have to be"
    );
    assert_eq!(
        Registry::Composer.search("psr log", 10),
        SearchSource {
            url: "https://packagist.org/search.json?q=psr%20log&per_page=10".to_owned(),
            accept: "application/json",
            whole_index: false,
            names_a_package: false,
        },
        "Packagist ranks a query itself, on its website's host rather than the repository's"
    );
}

#[test]
fn only_a_source_that_names_a_package_has_no_hits_where_it_has_nothing() {
    assert!(
        Registry::Go
            .search("github.com/go-chi/chi/v5", 10)
            .names_a_package,
        "a 404 from the proxy is a module path with nothing at it"
    );
    for registry in [Registry::Npm, Registry::Crates, Registry::PyPi] {
        assert!(
            !registry.search("requests", 10).names_a_package,
            "a 404 from {} is its search having moved, because no package was named",
            registry.id()
        );
    }
}

#[test]
fn a_go_search_answer_is_the_module_asked_for_at_the_version_latest_resolves() {
    let body = r#"{"Version":"v5.5.0","Time":"2026-10-14T13:20:04Z"}"#;

    assert_eq!(
        Registry::Go.read_hits(body, "github.com/go-chi/chi/v5", 10),
        Some(vec![Hit {
            name: "github.com/go-chi/chi/v5".to_owned(),
            version: Some("v5.5.0".to_owned()),
            description: None,
        }]),
        "the proxy says which version and nothing about what the module is"
    );
    assert_eq!(
        Registry::Go.read_hits("not found", "github.com/go-chi/chi/v5", 10),
        None,
        "a body that is not the proxy's answer is not a hit"
    );
}

#[test]
fn only_a_source_that_is_the_whole_index_says_so() {
    assert!(
        Registry::PyPi.search("requests", 10).whole_index,
        "PyPI's source is every name it publishes, whatever was asked"
    );
    for registry in [Registry::Npm, Registry::Crates] {
        assert!(
            !registry.search("requests", 10).whole_index,
            "{} is asked the query itself",
            registry.id()
        );
    }
}

#[test]
fn a_source_says_what_to_ask_it_for() {
    assert_eq!(
        Registry::Npm.search("zod", 10).accept,
        "application/json",
        "npm's search endpoint answers JSON and nothing else"
    );
    assert_eq!(
        Registry::PyPi.search("requests", 10).accept,
        "application/vnd.pypi.simple.v1+json",
        "and PyPI's index is HTML unless this asks otherwise"
    );
}

#[test]
fn a_source_is_never_asked_for_more_than_it_answers() {
    assert_eq!(
        Registry::Npm.search("zod", 1_000),
        SearchSource {
            url: "https://registry.npmjs.org/-/v1/search?text=zod&size=250".to_owned(),
            accept: "application/json",
            whole_index: false,
            names_a_package: false,
        },
        "npm refuses a size over 250"
    );
    assert_eq!(
        Registry::Crates.search("serde", 1_000),
        SearchSource {
            url: "https://crates.io/api/v1/crates?q=serde&per_page=100".to_owned(),
            accept: "application/json",
            whole_index: false,
            names_a_package: false,
        },
        "crates.io refuses a per_page over 100"
    );
    assert_eq!(
        Registry::Composer.search("monolog", 1_000).url,
        "https://packagist.org/search.json?q=monolog&per_page=100",
        "Packagist refuses a per_page over 100"
    );
    assert_eq!(
        Registry::Composer.search("monolog", 0).url,
        "https://packagist.org/search.json?q=monolog&per_page=1",
        "and one under 1"
    );
}

#[test]
fn npms_search_answer_reads_as_hits() {
    let body = r#"{"objects":[
        {"package":{"name":"zod","version":"4.0.0","description":"TypeScript-first schema validation"}},
        {"package":{"name":"zod-to-json-schema","version":"3.23.0","description":"Converts Zod schemas to JSON schemas"}}
    ],"total":2}"#;

    assert_eq!(
        Registry::Npm.read_hits(body, "zod", 10),
        Some(vec![
            Hit {
                name: "zod".to_owned(),
                version: Some("4.0.0".to_owned()),
                description: Some("TypeScript-first schema validation".to_owned()),
            },
            Hit {
                name: "zod-to-json-schema".to_owned(),
                version: Some("3.23.0".to_owned()),
                description: Some("Converts Zod schemas to JSON schemas".to_owned()),
            },
        ]),
        "npm carries all three fields, in the order it ranked them"
    );
}

#[test]
fn crates_ios_search_answer_reads_as_hits() {
    let body = r#"{"crates":[
        {"id":"serde","name":"serde","default_version":"1.0.229","newest_version":"2.0.0-alpha.1",
         "max_stable_version":"1.0.229","description":"A generic serialization/deserialization framework"}
    ],"meta":{"total":1}}"#;

    assert_eq!(
        Registry::Crates.read_hits(body, "serde", 10),
        Some(vec![Hit {
            name: "serde".to_owned(),
            version: Some("1.0.229".to_owned()),
            description: Some("A generic serialization/deserialization framework".to_owned()),
        }]),
        "a hit carries the version the registry itself would hand a caller"
    );
}

#[test]
fn packagists_search_answer_reads_as_hits_without_a_version() {
    let body = r#"{"results":[
        {"name":"monolog/monolog","description":"Sends your logs to files, sockets, inboxes, databases and various web services",
         "url":"https://packagist.org/packages/monolog/monolog","repository":"https://github.com/Seldaek/monolog",
         "downloads":1091256792,"favers":22270},
        {"name":"symfony/monolog-bundle","description":"Symfony MonologBundle",
         "url":"https://packagist.org/packages/symfony/monolog-bundle","repository":"https://github.com/symfony/monolog-bundle",
         "downloads":282942043,"favers":2928}
    ],"total":1170,"next":"https://packagist.org/search.json?q=monolog&page=2&per_page=2"}"#;

    assert_eq!(
        Registry::Composer.read_hits(body, "monolog", 10),
        Some(vec![
            Hit {
                name: "monolog/monolog".to_owned(),
                version: None,
                description: Some(
                    "Sends your logs to files, sockets, inboxes, databases and various web services"
                        .to_owned()
                ),
            },
            Hit {
                name: "symfony/monolog-bundle".to_owned(),
                version: None,
                description: Some("Symfony MonologBundle".to_owned()),
            },
        ]),
        "Packagist's search carries a name and a description, in its own ranking, and no version"
    );
    assert_eq!(
        Registry::Composer.read_hits("<html></html>", "monolog", 10),
        None
    );
}

#[test]
fn a_packagist_hit_that_only_names_what_other_packages_provide_is_left_out() {
    let body = r#"{"results":[
        {"name":"psr/log-implementation","description":"",
         "url":"https://packagist.org/providers/psr/log-implementation","repository":"","virtual":true},
        {"name":"monolog/monolog","description":"Sends your logs to files, sockets, inboxes, databases and various web services",
         "url":"https://packagist.org/packages/monolog/monolog","repository":"https://github.com/Seldaek/monolog",
         "downloads":1091256792,"favers":22270}
    ],"total":2}"#;

    let names: Vec<String> = Registry::Composer
        .read_hits(body, "log implementation", 10)
        .expect("a Packagist search answer reads")
        .into_iter()
        .map(|hit| hit.name)
        .collect();

    assert_eq!(
        names,
        vec!["monolog/monolog".to_owned()],
        "a virtual package has no versions and no archive, so no other tool could take its name"
    );
}

#[test]
fn pypis_index_reads_as_hits_the_query_matches() {
    let body = r#"{"meta":{"api-version":"1.4"},"projects":[
        {"name":"requests"},
        {"name":"ruamel.yaml"},
        {"name":"yamllint"},
        {"name":"PyYAML"},
        {"name":"yaml"},
        {"name":"yamldown"}
    ]}"#;

    let names: Vec<String> = Registry::PyPi
        .read_hits(body, "yaml", 10)
        .expect("the index reads")
        .into_iter()
        .map(|hit| hit.name)
        .collect();

    assert_eq!(
        names,
        vec!["yaml", "yamldown", "yamllint", "PyYAML", "ruamel.yaml"],
        "the name itself, then what starts with it, then what contains it"
    );
}

#[test]
fn a_pypi_hit_carries_the_name_the_index_carries_and_no_more() {
    let body = r#"{"meta":{"api-version":"1.4"},"projects":[{"name":"requests"}]}"#;

    assert_eq!(
        Registry::PyPi.read_hits(body, "requests", 10),
        Some(vec![Hit {
            name: "requests".to_owned(),
            version: None,
            description: None,
        }])
    );
}

#[test]
fn a_registrys_hosts_are_the_hosts_of_the_urls_it_builds() {
    for registry in Registry::ALL {
        let hosts = registry.hosts();
        let mut built = vec![
            registry
                .archive("vendor/package", "1.0.0")
                .expect("a registry says where its archives are")
                .url()
                .to_owned(),
            registry.versions("vendor/package").url,
        ];
        built.push(registry.search("query", 1).url);

        for url in built {
            let host = url
                .strip_prefix("https://")
                .and_then(|rest| rest.split('/').next())
                .expect("this module builds https URLs")
                .to_owned();
            assert!(
                hosts.contains(&host),
                "{} builds {url} but does not allow {host}, got {hosts:?}",
                registry.id()
            );
        }
    }
}

#[test]
fn a_host_that_only_a_listing_names_is_allowed_too() {
    assert!(
        Registry::PyPi.hosts().contains("files.pythonhosted.org"),
        "PyPI archives come from the file host, got {:?}",
        Registry::PyPi.hosts()
    );
}

#[test]
fn packagist_archives_come_from_the_hosts_its_listings_name_and_never_the_github_api() {
    assert_eq!(
        Registry::Composer.hosts(),
        [
            "bitbucket.org",
            "codeload.github.com",
            "gitlab.com",
            "packagist.org",
            "repo.packagist.org",
        ]
        .into_iter()
        .map(str::to_owned)
        .collect(),
        "the engine turns a GitHub zipball into its codeload URL, so `api.github.com` \
         is never asked"
    );
}

#[test]
fn the_allowlist_is_the_union_and_grows_with_the_registries_in_it() {
    assert_eq!(
        registry::allowed_hosts(),
        registry::hosts_of(&Registry::ALL),
        "the allowlist is every registry's hosts and nothing else"
    );

    let npm_alone = registry::hosts_of(&[Registry::Npm]);
    let with_pypi = registry::hosts_of(&[Registry::Npm, Registry::PyPi]);

    assert!(
        npm_alone.is_subset(&with_pypi) && npm_alone.len() < with_pypi.len(),
        "a registry added to the list adds its hosts: {npm_alone:?} then {with_pypi:?}"
    );
}

#[test]
fn a_host_outside_the_allowlist_is_refused() {
    assert!(
        registry::allows("https://registry.npmjs.org/zod/-/zod-4.0.0.tgz"),
        "the host an archive URL names is reachable"
    );
    assert!(
        registry::allows("https://files.pythonhosted.org/packages/x-1.0.tar.gz"),
        "and so is the one a PyPI listing names"
    );

    for refused in [
        "https://evil.example/zod-4.0.0.tgz",
        "https://registry.npmjs.org.evil.example/zod-4.0.0.tgz",
        "https://evil.example/registry.npmjs.org/zod-4.0.0.tgz",
        "https://registry.npmjs.org@evil.example/zod-4.0.0.tgz",
        "https://registry.npmjs.org:8443/zod-4.0.0.tgz",
        "http://registry.npmjs.org/zod-4.0.0.tgz",
        "file:///etc/passwd",
        "registry.npmjs.org/zod-4.0.0.tgz",
    ] {
        assert!(
            !registry::allows(refused),
            "`{refused}` is not a registry this server may reach"
        );
    }
}

#[test]
fn every_registry_states_the_name_rule_that_trips_an_agent() {
    let rules: Vec<&str> = Registry::ALL
        .iter()
        .map(|registry| registry.name_rule())
        .collect();

    for (registry, rule) in Registry::ALL.iter().zip(&rules) {
        assert!(
            !rule.is_empty(),
            "{} should say how it spells a package name",
            registry.id()
        );
    }
    assert_eq!(
        rules
            .iter()
            .collect::<std::collections::BTreeSet<_>>()
            .len(),
        rules.len(),
        "three registries with the same rule would mean two of them are guesses, got {rules:?}"
    );

    assert!(
        Registry::Npm.name_rule().contains("@types/node"),
        "npm's rule is the scoped name, spelled out: {}",
        Registry::Npm.name_rule()
    );
    assert!(
        Registry::PyPi.name_rule().contains("PEP 503"),
        "PyPI's rule is the normalisation this server does not do: {}",
        Registry::PyPi.name_rule()
    );
    assert!(
        Registry::Composer.name_rule().contains("`monolog/monolog`")
            && Registry::Composer.name_rule().contains("lower case"),
        "Packagist's rule is the vendor, the package and their case: {}",
        Registry::Composer.name_rule()
    );
}

#[test]
fn a_version_is_stated_to_be_exact_wherever_a_name_rule_is_read() {
    assert!(
        registry::VERSION_RULE.contains("range"),
        "the rule has to rule a range out by name, got {}",
        registry::VERSION_RULE
    );
}
