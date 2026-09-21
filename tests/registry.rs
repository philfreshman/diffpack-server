//! What a registry is, asked of the one module that knows.
//!
//! Every expectation here comes from [`CONTEXT.md`](../CONTEXT.md) or from
//! the issue that owns the fact — #10 for the archive URLs, #18 for the
//! version sources and their orderings, #19 for search — and never from
//! running the code and recording what it said. A test that agreed with the
//! implementation by construction would let all five copies this module
//! replaces come back one field at a time.
//!
//! Nothing here reaches the network, which is the property that makes every
//! registry fact cheap to assert: this module says *where* and *what shape*,
//! and `archive` (#10) is what fetches.

use diffpack_server::registry::{
    self, ArchiveSource, Hit, Order, Registry, SearchSource, VersionSource,
};

/// The identifier is what a parameter and a cache key spell; the name is what
/// a message to a model says. They are deliberately different strings —
/// `crates` and `crates.io` — and CONTEXT.md is where the pairing is fixed.
#[test]
fn every_registry_carries_the_identifier_and_the_name_it_is_known_by() {
    let known: Vec<(&str, &str)> = Registry::ALL
        .iter()
        .map(|registry| (registry.id(), registry.name()))
        .collect();

    assert_eq!(
        known,
        vec![("npm", "npm"), ("crates", "crates.io"), ("pypi", "PyPI")],
        "the three registries of CONTEXT.md, in the order a catalogue lists them"
    );
}

/// The identifier is also how a registry arrives: a tool is given `crates`,
/// not a `Registry`. Round-tripping every variant is what keeps the parser
/// and [`Registry::id`] from disagreeing about a spelling, which is the
/// failure that makes a package unreachable rather than a compile error.
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
        Registry::from_id("go"),
        None,
        "Go is #28's to add, and until it is added it is not a registry"
    );
}

/// A registry crosses the wire as its identifier and is read back the same
/// way, so the JSON a tool is called with, the JSON `diffpack://registries`
/// (#16) serves and the value in a cache key are one spelling rather than
/// three.
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

/// An identifier that is not one of ours is refused, and the refusal names
/// the ones that are: the message reaches a client as `-32602` through
/// [`Failure::InvalidParams`](diffpack_server::error::Failure), and a client
/// that is told only "unknown variant" has to guess what would have worked.
#[test]
fn an_unknown_identifier_is_refused_by_naming_the_ones_that_exist() {
    let refusal = serde_json::from_value::<Registry>(serde_json::json!("go"))
        .expect_err("`go` is #28's to add")
        .to_string();

    for known in Registry::ALL {
        assert!(
            refusal.contains(known.id()),
            "the refusal should name `{}`, got {refusal}",
            known.id()
        );
    }
}

/// The `registry` enum a tool declares is this module's, generated from
/// [`Registry::ALL`] rather than written out beside each tool. A fourth
/// registry is then a variant, not a schema somebody has to remember to
/// widen — and a schema that promised a registry the server cannot parse
/// would be a tool advertising a call that always fails.
#[test]
fn the_schema_lists_exactly_the_registries_this_server_has() {
    let schema = serde_json::to_value(schemars::schema_for!(Registry)).expect("a schema is JSON");

    assert_eq!(
        schema["type"], "string",
        "a registry is a string on the wire, got {schema}"
    );
    assert_eq!(
        schema["enum"],
        serde_json::json!(["npm", "crates", "pypi"]),
        "the schema's values are the identifiers, in the catalogue's order"
    );
}

// ---------------------------------------------------------------------------
// Where a version's archive is
// ---------------------------------------------------------------------------

/// npm and crates.io publish an archive at a path anyone can construct; PyPI
/// lists a version's files in its own metadata and nowhere else. Both are
/// answers to the same question, which is why they are one type: a caller
/// asks where the archive is and is told either the URL or what to fetch
/// first, rather than learning that PyPI is special.
///
/// The URLs are #10's, and the npm rule is the one that is not obvious — the
/// path keeps the scope and the filename drops it.
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
}

/// The second half of the PyPI hop: which of a version's files to take. A
/// source distribution is what a diff wants — a wheel is built output — and
/// the preference order is the engine's, so the browser and the server pick
/// the same file for the same version.
#[test]
fn a_listing_is_read_for_the_archive_a_diff_wants() {
    let listing = |files: &str| format!(r#"{{"urls":[{files}]}}"#);
    let sdist = r#"{"packagetype":"sdist","url":"https://files.pythonhosted.org/x-1.0.tar.gz"}"#;
    let wheel = r#"{"packagetype":"bdist_wheel","url":"https://files.pythonhosted.org/x-py3-none-any.whl"}"#;

    assert_eq!(
        Registry::PyPi.choose_archive(&listing(&format!("{wheel},{sdist}"))),
        Some("https://files.pythonhosted.org/x-1.0.tar.gz".to_owned()),
        "an sdist is preferred over a wheel, whichever order they are listed in"
    );
    assert_eq!(
        Registry::PyPi.choose_archive(&listing(wheel)),
        Some("https://files.pythonhosted.org/x-py3-none-any.whl".to_owned()),
        "a version published only as a wheel is still diffable"
    );
    assert_eq!(
        Registry::PyPi.choose_archive(&listing("")),
        None,
        "a version with no file this server can read names no archive"
    );
    assert_eq!(
        Registry::PyPi.choose_archive("not metadata"),
        None,
        "and neither does a body that is not the metadata we asked for"
    );
}

// ---------------------------------------------------------------------------
// Where the rest of a registry's answers come from
// ---------------------------------------------------------------------------

/// #18 asks every registry for versions newest-first, and only one of the
/// three answers that way already. The source and the direction travel
/// together because apart they are a table in an issue: a tool told where to
/// ask and left to remember which way the answer runs is a tool that lists
/// npm backwards.
#[test]
fn every_registry_says_where_versions_come_from_and_which_way_they_run() {
    assert_eq!(
        Registry::Npm.versions("zod"),
        VersionSource {
            url: "https://registry.npmjs.org/zod".to_owned(),
            order: Order::OldestFirst,
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
            order: Order::NewestFirst,
        },
        "crates.io is the one source that already answers the way #18 wants"
    );
    assert_eq!(
        Registry::PyPi.versions("requests"),
        VersionSource {
            url: "https://api.deps.dev/v3/systems/pypi/packages/requests".to_owned(),
            order: Order::OldestFirst,
        },
        "PyPI's own index is not a version list a client can read, so deps.dev is the source"
    );
}

/// Where a search is asked. npm and crates.io answer a query from an
/// endpoint anyone can build; PyPI answers none, so what it is asked for is
/// the index of everything it publishes and the query is applied on this
/// side. #19 records why that is the source and what the alternatives cost.
///
/// The URL is the whole of what a caller is told, which is why PyPI's
/// carries no query: a source that cannot be asked a question is still a
/// source, and reading its answer is `read_hits`'s half of the job.
#[test]
fn search_comes_from_the_registrys_own_index() {
    assert_eq!(
        Registry::Npm.search("zod", 10),
        SearchSource {
            url: "https://registry.npmjs.org/-/v1/search?text=zod&size=10".to_owned(),
        }
    );
    assert_eq!(
        Registry::Crates.search("json parser", 5),
        SearchSource {
            url: "https://crates.io/api/v1/crates?q=json%20parser&per_page=5".to_owned(),
        },
        "a query is a parameter value, so a space in it is escaped and not sent"
    );
    assert_eq!(
        Registry::PyPi.search("requests", 10),
        SearchSource {
            url: "https://pypi.org/simple/".to_owned(),
        },
        "PyPI has no search endpoint, so the source is the index itself"
    );
}

/// What a search answers with is this module's to read, for the same reason
/// where to ask it is: the three sources agree about nothing — npm wraps a
/// package in an `objects` array, crates.io returns `crates`, and PyPI's
/// index is a list of names with no version and no summary anywhere in it. A
/// caller left to tell them apart would be the per-registry `match` this
/// module exists to hold.
///
/// The body below is npm's own shape, cut down to the three fields a hit
/// carries. #19 is where the fields are fixed.
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

/// crates.io answers with `crates`, and with three version fields that do not
/// have to agree. The one a hit carries is `default_version` — what the
/// registry hands a caller that did not ask for a version, which is the same
/// thing npm's `version` is. `newest_version` would answer a search for a
/// crate whose latest release is a pre-release with a version nobody is meant
/// to install yet.
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

// ---------------------------------------------------------------------------
// The outbound allowlist
// ---------------------------------------------------------------------------

/// A registry's hosts are the hosts of the URLs it builds, which is what
/// makes the allowlist a consequence of this module rather than a second list
/// beside it. A source added above without its host reaching this set is a
/// registry that is allowed to be asked and not allowed to answer, and that
/// failure looks like a broken registry — which is how allowlists get widened
/// until they allow everything.
#[test]
fn a_registrys_hosts_are_the_hosts_of_the_urls_it_builds() {
    for registry in Registry::ALL {
        let hosts = registry.hosts();
        let mut built = vec![
            registry
                .archive("package", "1.0.0")
                .expect("a registry says where its archives are")
                .url()
                .to_owned(),
            registry.versions("package").url,
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

/// PyPI's archives are served from a host no URL here names: the address
/// comes out of the metadata at request time. It is declared beside the
/// registry that needs it, so that the allowlist still covers everything the
/// fetch path reaches.
#[test]
fn a_host_that_only_a_listing_names_is_allowed_too() {
    assert!(
        Registry::PyPi.hosts().contains("files.pythonhosted.org"),
        "PyPI archives come from the file host, got {:?}",
        Registry::PyPi.hosts()
    );
}

/// The allowlist is the union over the registries there are, so adding one
/// adds its hosts and no edit anywhere else. This is the property the issue
/// asks to be proven rather than asserted: a list that has to be widened by
/// hand is a list that is one registry behind.
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

/// The point of the allowlist: a registry name arriving in a tool argument
/// must never become a request to an arbitrary host. Each of these is a way
/// of looking like a registry without being one.
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

// ---------------------------------------------------------------------------
// The rules an agent cannot guess
// ---------------------------------------------------------------------------

/// #23's point: a name is taken verbatim, and what "verbatim" costs differs
/// per registry. An agent that assumes npm names are one segment sends half
/// of a scoped name, and one that assumes PyPI names are normalised asks for
/// `typing-extensions` when the package is `Typing.Extensions`. Each
/// registry says its own rule, and `diffpack://registries` (#16) is where an
/// agent reads them.
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
}

/// The rule that is not per-registry: a version is one published version, as
/// the registry spells it. It sits beside the three because it is read in the
/// same breath — an agent handed a name rule and left to guess whether a
/// range works will try one.
#[test]
fn a_version_is_stated_to_be_exact_wherever_a_name_rule_is_read() {
    assert!(
        registry::VERSION_RULE.contains("range"),
        "the rule has to rule a range out by name, got {}",
        registry::VERSION_RULE
    );
}
