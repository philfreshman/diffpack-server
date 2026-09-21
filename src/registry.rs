//! What a registry is.
//!
//! One concept, defined once: the identifier a parameter and a cache key
//! spell (`npm`, `crates`, `pypi`), the name the registry calls itself in a
//! message a model reads (`npm`, `crates.io`, `PyPI`), and where to reach it.
//! Everything that takes a `registry` argument parses it through
//! [`Registry`], and the `diffpack://registries` resource (#16) serialises
//! this module rather than describing it a second time. See [ADR
//! 0004](../docs/adr/0004-one-registry-module.md).
//!
//! # Why a `match` and not a trait
//!
//! Every per-registry fact below is a `match` over three variants in this
//! file, rather than a `Registry` trait with an implementation each. Three
//! arms of one function sit next to each other, so the question a reader
//! actually has — *how does PyPI differ from npm here?* — is answered by
//! reading down a screen. Split across three types it is answered by opening
//! three files and holding them in your head. The trait would earn its keep
//! if a registry could arrive from outside this crate; none can, because a
//! registry is a value in a tool's schema.

use std::borrow::Cow;
use std::collections::BTreeSet;

use schemars::{json_schema, JsonSchema, Schema, SchemaGenerator};
use serde::{de, Deserialize, Deserializer, Serialize, Serializer};

use crate::engine;
use crate::error::Failure;

/// A package host this server knows.
///
/// The single definition every tool schema derives its `registry` enum from.
/// Adding one — Go, in #28 — is a variant here and an arm in each `match`
/// below, which the compiler enumerates; it is not an edit in five files
/// somebody has to find first.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Registry {
    Npm,
    Crates,
    PyPi,
}

impl Registry {
    /// Every registry, in the order a catalogue lists them.
    ///
    /// The order is fixed so that `diffpack://registries` and any message
    /// built from this list are stable between deploys, and it is the only
    /// list: a derivation over registries iterates this rather than repeating
    /// the variants.
    pub const ALL: [Registry; 3] = [Registry::Npm, Registry::Crates, Registry::PyPi];

    /// The identifier as the wire spells it: `npm`, `crates`, `pypi`.
    ///
    /// This is what arrives in a tool argument and what goes into a
    /// [`DiffKey`](crate::cache_key::DiffKey), so it is not free to change —
    /// a different spelling here is a cache that misses every entry it
    /// already holds.
    pub fn id(self) -> &'static str {
        match self {
            Self::Npm => "npm",
            Self::Crates => "crates",
            Self::PyPi => "pypi",
        }
    }

    /// The registry an identifier names, or nothing.
    ///
    /// The inverse of [`id`](Self::id), over [`ALL`](Self::ALL) rather than a
    /// second `match`: a variant added to the enum is parseable the moment
    /// `id` has an arm for it, so there is no table here to forget.
    ///
    /// Nothing is normalised on the way in — not case, not `crates.io` for
    /// `crates` — for the same reason a package name is not: the identifier
    /// is a field in a cache key, and two spellings that both parsed would
    /// be two keys for one diff.
    pub fn from_id(id: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|registry| registry.id() == id)
    }

    /// The name the registry uses for itself: `npm`, `crates.io`, `PyPI`.
    ///
    /// What a message to a model says ([`crate::error::Failure`]) and what a
    /// catalogue shows a person (#16) — one string for both, because a
    /// separate display label would hold the same three values with nothing
    /// to tell them apart, which is a copy that drifts. Distinct from
    /// [`id`](Self::id) on purpose: a model told `crates` where it expected
    /// `crates.io` has to wonder whether those are two registries.
    pub fn name(self) -> &'static str {
        match self {
            Self::Npm => "npm",
            Self::Crates => "crates.io",
            Self::PyPi => "PyPI",
        }
    }

    /// How to reach `version` of `package`.
    ///
    /// Either the archive's URL or the metadata document that names it — one
    /// type, because both answer the same question and a caller that had to
    /// know which registry needs a second hop would be carrying this module's
    /// job around with it.
    ///
    /// The URLs come from [`crate::engine`], which is the code the browser
    /// runs. A pattern written out here would be a second copy of a string
    /// that has to keep matching what the web app fetches.
    ///
    /// The `Err` is unreachable by construction — it is the engine refusing a
    /// registry identifier this module handed it, which is a disagreement
    /// between two files in this repository rather than anything a caller
    /// did, so it takes the internal channel rather than a panic.
    pub fn archive(self, package: &str, version: &str) -> Result<ArchiveSource, Failure> {
        match self {
            Self::Npm | Self::Crates => engine::build_tarball_url(self.id(), package, version)
                .map(|url| ArchiveSource::Archive { url })
                .map_err(|_| Failure::Internal {
                    doing: "building an archive URL",
                }),

            // Two hops, and the reason PyPI cannot answer #10's
            // `resolve_archive_url` from its arguments alone.
            Self::PyPi => Ok(ArchiveSource::Listing {
                url: format!("https://pypi.org/pypi/{package}/{version}/json"),
            }),
        }
    }

    /// How this registry spells a package name, for a reader who will
    /// otherwise guess.
    ///
    /// A name is taken verbatim here — the same rule `docs/cache-key.md`
    /// fixes for the key — and what that costs differs per registry, which is
    /// exactly what #23 says an agent cannot work out for itself. These are
    /// the sentences `diffpack://registries` (#16) serves and a tool's schema
    /// can quote.
    pub fn name_rule(self) -> &'static str {
        match self {
            Self::Npm => {
                "A scoped name is one package name, `@` and `/` included: \
                          `@types/node`, not `types/node` and not `node`."
            }
            Self::Crates => {
                "A crate name is one segment, and `-` and `_` are different \
                             names: `serde_json` is not `serde-json`."
            }
            Self::PyPi => {
                "The name is spelled as PyPI spells it. No PEP 503 \
                           normalisation is applied, so `Typing.Extensions` keeps its \
                           case and its dot."
            }
        }
    }

    /// Where `package`'s versions are listed.
    ///
    /// The package name is escaped rather than interpolated: a scoped npm
    /// name is one package name, and `@types/node` written into a path
    /// unescaped is a request for a package called `node` inside a scope.
    pub fn versions(self, package: &str) -> VersionSource {
        let escaped = escape(package);
        match self {
            Self::Npm => VersionSource {
                url: format!("https://registry.npmjs.org/{escaped}"),
            },
            Self::Crates => VersionSource {
                url: format!("https://crates.io/api/v1/crates/{escaped}"),
            },
            // PyPI's own JSON names a package's releases but without the
            // dates that order them, and its simple index is HTML. deps.dev
            // is what the web app already reads for the same reason, so the
            // two agree about what versions a package has.
            Self::PyPi => VersionSource {
                url: format!("https://api.deps.dev/v3/systems/pypi/packages/{escaped}"),
            },
        }
    }

    /// The versions a [`VersionSource`] body names, newest first, or nothing
    /// if this server cannot read it.
    ///
    /// # Why the order is computed here rather than declared
    ///
    /// This used to be a field beside the URL saying which way each source
    /// runs. It was wrong, and wrong in a way no amount of reversing fixes:
    /// deps.dev sorts PyPI's versions *lexically by version string*, so
    /// `requests` ends at `2.9.2` and reversing it reports that as the
    /// newest release rather than `2.34.2`. npm's is worse — its versions
    /// are a JSON object, and `serde_json`'s map is a `BTreeMap` here, so
    /// the document's own order is gone before this crate ever sees it.
    ///
    /// All three documents carry a publish date per version, so that is what
    /// newest first means: most recently published first. It is not the
    /// highest version number — npm's `@types/node` publishes a 22.x patch
    /// after a 26.x release most weeks, and both registries' own listings
    /// show the patch on top.
    ///
    /// The dates are compared as the strings the registry wrote. Each source
    /// spells them one way, so ordering within one answer is exact, and no
    /// calendar has to be parsed to sort releases.
    ///
    /// `None` rather than an error, for the same reason
    /// [`choose_archive`](Self::choose_archive) is: a document this server
    /// cannot read is something the caller has to explain to a model, and
    /// the caller is the one holding the package name that belongs in that
    /// message.
    pub fn read_versions(self, document: &str) -> Option<Vec<Version>> {
        let mut versions = match self {
            Self::Npm => {
                // Only the keys are wanted from `versions`; the dates are in
                // `time`, which also carries `created` and `modified`. The
                // intersection is the point: `time` alone would invent two
                // versions, and `versions` alone has no order.
                #[derive(Deserialize)]
                struct Document {
                    versions: std::collections::BTreeMap<String, de::IgnoredAny>,
                    time: std::collections::BTreeMap<String, String>,
                }

                let document: Document = serde_json::from_str(document).ok()?;
                document
                    .versions
                    .into_keys()
                    .map(|version| Version {
                        prerelease: self.is_prerelease(&version),
                        published_at: document.time.get(&version).cloned(),
                        version,
                    })
                    .collect::<Vec<_>>()
            }

            Self::Crates => {
                #[derive(Deserialize)]
                struct Document {
                    versions: Vec<Release>,
                }
                #[derive(Deserialize)]
                struct Release {
                    num: String,
                    created_at: Option<String>,
                }

                let document: Document = serde_json::from_str(document).ok()?;
                document
                    .versions
                    .into_iter()
                    .map(|release| Version {
                        prerelease: self.is_prerelease(&release.num),
                        version: release.num,
                        published_at: release.created_at,
                    })
                    .collect::<Vec<_>>()
            }

            // deps.dev sorts these lexically by version string, which is
            // neither end of the list: `requests` runs to 2.9.2 because
            // "2.9.2" sorts after "2.34.2". The dates are the only thing in
            // this document that puts releases in order.
            Self::PyPi => {
                #[derive(Deserialize)]
                struct Document {
                    versions: Vec<Release>,
                }
                #[derive(Deserialize)]
                #[serde(rename_all = "camelCase")]
                struct Release {
                    version_key: VersionKey,
                    #[serde(default)]
                    published_at: Option<String>,
                }
                #[derive(Deserialize)]
                struct VersionKey {
                    version: String,
                }

                let document: Document = serde_json::from_str(document).ok()?;
                document
                    .versions
                    .into_iter()
                    .map(|release| Version {
                        prerelease: self.is_prerelease(&release.version_key.version),
                        version: release.version_key.version,
                        published_at: release.published_at,
                    })
                    .collect::<Vec<_>>()
            }
        };

        // Newest first, and by the date rather than by the name. A version
        // the source gave no date for sorts last, which is what reversing
        // `Option`'s own order does — `None` is less than every `Some` — and
        // is the only honest place for it: the promise is newest first, and a
        // release this server cannot date is not one it can call the newest.
        //
        // Ties keep whatever order they arrived in, which for two releases
        // published in the same instant is not a question anyone is asking.
        versions.sort_by(|a, b| b.published_at.cmp(&a.published_at));
        Some(versions)
    }

    /// Where a search for `query` is answered, and for at most `limit` hits.
    ///
    /// The limit is in the URL rather than left to a caller because the
    /// parameter is the registry's own spelling — `size` on npm, `per_page`
    /// on crates.io — and a caller that had to know which is which would be
    /// the per-registry `match` this module exists to remove.
    ///
    /// PyPI has no search endpoint at all: its XML-RPC search was withdrawn
    /// in 2021 and its search page is a web page its own `robots.txt` asks
    /// automated clients not to fetch. So its source is the index itself —
    /// every name it publishes, PEP 691's JSON form — and neither the query
    /// nor the limit can be put to it. Both are applied on this side, in
    /// [`read_hits`](Self::read_hits). #19 records what the alternatives
    /// cost.
    pub fn search(self, query: &str, limit: u32) -> SearchSource {
        let escaped = escape(query);
        let (url, accept, whole_index) = match self {
            // The most each source answers, in that source's own units.
            // Narrowed here rather than sent: npm and crates.io both refuse a
            // larger one outright, so a caller asking for a thousand hits
            // would get an error instead of the hundred that exist.
            Self::Npm => {
                let size = limit.min(250);
                (
                    format!("https://registry.npmjs.org/-/v1/search?text={escaped}&size={size}"),
                    "application/json",
                    false,
                )
            }
            Self::Crates => {
                let per_page = limit.min(100);
                (
                    format!("https://crates.io/api/v1/crates?q={escaped}&per_page={per_page}"),
                    "application/json",
                    false,
                )
            }
            // Without this the same URL answers with the web page pip does
            // not read either.
            Self::PyPi => (
                "https://pypi.org/simple/".to_owned(),
                "application/vnd.pypi.simple.v1+json",
                true,
            ),
        };
        SearchSource {
            url,
            accept,
            whole_index,
        }
    }

    /// The hits a search answer names, or nothing if it is not an answer
    /// this registry's source gives.
    ///
    /// The other half of [`search`](Self::search), and here for the reason
    /// that one is: the three sources agree about nothing. npm wraps each
    /// package in an `objects` array, crates.io answers with `crates`, and
    /// PyPI's index is every name it publishes and no versions at all. A
    /// caller that had to tell them apart would be carrying this module's
    /// job.
    ///
    /// `query` is here for the source that does not take one. npm and
    /// crates.io are asked the query and answer it; PyPI's index is the
    /// whole list, so the matching happens on this side and the query is
    /// what it matches against.
    ///
    /// `None` rather than an error, as [`choose_archive`](Self::choose_archive)
    /// does: a body that will not read is the source serving something
    /// broken, and the caller is the one holding the registry and the query
    /// that belong in that message.
    pub fn read_hits(self, body: &str, query: &str, limit: u32) -> Option<Vec<Hit>> {
        let hits: Vec<Hit> = match self {
            Self::Npm => {
                let answer: NpmSearch = serde_json::from_str(body).ok()?;
                answer
                    .objects
                    .into_iter()
                    .map(|object| Hit {
                        name: object.package.name,
                        version: object.package.version,
                        description: object.package.description,
                    })
                    .collect()
            }
            Self::Crates => {
                let answer: CratesSearch = serde_json::from_str(body).ok()?;
                answer
                    .crates
                    .into_iter()
                    .map(|found| Hit {
                        name: found.name,
                        // Three version fields arrive and they do not have to
                        // agree. This is the one the registry itself would
                        // hand a caller that named no version, which is what
                        // npm's `version` is too — `newest_version` would
                        // answer with a pre-release nobody is meant to
                        // install yet.
                        version: found.default_version,
                        description: found.description,
                    })
                    .collect()
            }

            // The index is every name PyPI publishes, so matching and
            // ordering happen here rather than at the source. `rank` is the
            // whole of the relevance this server claims, and it is written
            // where the reader asking "how does PyPI differ here?" is
            // already looking.
            Self::PyPi => {
                let index: PyPiIndex = serde_json::from_str(body).ok()?;
                let query = query.to_lowercase();

                let mut matched: Vec<(Rank, &str)> = index
                    .projects
                    .iter()
                    .filter_map(|project| {
                        rank(&project.name, &query).map(|rank| (rank, project.name.as_str()))
                    })
                    .collect();

                // Length before spelling: of two names that both start with
                // the query, the shorter is the one that is mostly the
                // query. Alphabetical is the tie-break rather than the rule,
                // so the order does not depend on the order the index
                // happened to list them in.
                matched.sort_by(|(left_rank, left), (right_rank, right)| {
                    left_rank
                        .cmp(right_rank)
                        .then_with(|| left.len().cmp(&right.len()))
                        .then_with(|| left.cmp(right))
                });

                matched
                    .into_iter()
                    .map(|(_, name)| Hit {
                        name: name.to_owned(),
                        // The index carries neither, for any project in it.
                        version: None,
                        description: None,
                    })
                    .collect()
            }
        };

        Some(hits.into_iter().take(limit as usize).collect())
    }

    /// Every host this registry is allowed to be reached at.
    ///
    /// Derived, not declared: the set is the hosts of the URLs this module
    /// builds for the registry — its archive, its versions, its search — so a
    /// source added above is reachable the moment it exists. A list typed out
    /// separately is a list that will one day be missing an entry the code
    /// needs, and the symptom is a broken registry, which is how an allowlist
    /// gets widened until it allows everything.
    ///
    /// [`listed_hosts`](Self::listed_hosts) is the part that cannot be
    /// derived, and it is small and next to the derivation for that reason.
    pub fn hosts(self) -> BTreeSet<String> {
        // A name and a version that produce a URL of the ordinary shape. What
        // they are does not matter: only the host is read back out, and every
        // package on a registry is served from the same one.
        const PROBE: &str = "package";

        let mut urls = vec![self.versions(PROBE).url];
        if let Ok(archive) = self.archive(PROBE, "1.0.0") {
            urls.push(archive.url().to_owned());
        }
        urls.push(self.search(PROBE, 1).url);

        let mut hosts: BTreeSet<String> = urls
            .iter()
            .filter_map(|url| host_of(url))
            .map(str::to_owned)
            .collect();
        hosts.extend(self.listed_hosts().iter().map(|host| (*host).to_owned()));
        hosts
    }

    /// The hosts this registry serves from that no URL here names.
    ///
    /// PyPI's archives are the case: the address arrives in the metadata at
    /// request time, so there is nothing to derive it from. Declaring it
    /// beside the registry that needs it keeps the allowlist complete without
    /// making the whole list hand-written.
    fn listed_hosts(self) -> &'static [&'static str] {
        match self {
            Self::Npm | Self::Crates => &[],
            Self::PyPi => &["files.pythonhosted.org"],
        }
    }

    /// Whether `version` is a preview rather than a release.
    ///
    /// A per-registry fact because the two spellings are genuinely different
    /// standards rather than dialects: npm and crates.io use semver, where a
    /// prerelease is what follows the first `-`, and PyPI uses PEP 440, where
    /// it is an `a`, `b`, `rc` or `dev` segment glued to the release with no
    /// separator required at all. `1.0rc1` is a release candidate on PyPI and
    /// is not a version on either of the other two.
    ///
    /// Worth telling an agent because "the last two versions" is the question
    /// this tool exists for, and the answer to it should not quietly be a
    /// release candidate.
    pub fn is_prerelease(self, version: &str) -> bool {
        // Build metadata is not a prerelease under either standard — semver's
        // `+build.5` and PEP 440's `+local` are labels on a release — and it
        // can contain anything, so it goes before either rule looks.
        let version = version.split('+').next().unwrap_or_default();

        match self {
            Self::Npm | Self::Crates => version.contains('-'),
            Self::PyPi => pep440_prerelease(version),
        }
    }

    /// The archive a [`ArchiveSource::Listing`] body names, if it names one
    /// this server can read.
    ///
    /// A source distribution is preferred over a wheel — a diff of built
    /// output is not the diff anyone asked for — and the preference order is
    /// the engine's, so the browser and the server choose the same file for
    /// the same version.
    ///
    /// `None` rather than an error: a version whose files this server cannot
    /// use is something the caller has to explain to a model, and the caller
    /// is the one holding the package name and the version that belong in
    /// that message.
    pub fn choose_archive(self, listing: &str) -> Option<String> {
        match self {
            // Their archive URL is built, not listed, so there is no
            // metadata document to read and nothing to choose from.
            Self::Npm | Self::Crates => None,

            Self::PyPi => {
                let metadata: engine::PyPiResponse = serde_json::from_str(listing).ok()?;
                engine::select_pypi_sdist_url(&metadata.urls).ok()
            }
        }
    }
}

/// Where a version's archive is.
///
/// The distinction is a registry's, not a caller's: npm and crates.io publish
/// at a path anyone can construct, and PyPI lists a version's files in its
/// own metadata. A tool that answers from its arguments alone (#10's
/// `resolve_archive_url`) can serve the first and not the second, and that is
/// the whole of what it has to know.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ArchiveSource {
    /// The archive itself, at a URL built from the package and the version.
    Archive { url: String },

    /// A metadata document listing the version's files. Fetch it, then read
    /// the archive out of it with [`Registry::choose_archive`].
    Listing { url: String },
}

impl ArchiveSource {
    /// The URL to fetch first, whichever kind of source this is.
    ///
    /// What is at the other end differs — an archive or a listing — and that
    /// is what the variant says. This is for the caller that only needs to
    /// know where the request goes, which is how the host allowlist is
    /// derived without a second copy of the patterns.
    pub fn url(&self) -> &str {
        match self {
            Self::Archive { url } | Self::Listing { url } => url,
        }
    }
}

/// On the wire a registry is its identifier.
///
/// Written by hand rather than derived so that there is one spelling of
/// `crates` in this crate and not two: a `#[serde(rename_all = …)]` beside
/// [`Registry::id`] agrees with it today and silently stops agreeing the
/// first time a variant's identifier is not its name lowercased.
impl Serialize for Registry {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.id())
    }
}

impl<'de> Deserialize<'de> for Registry {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        // The refusal names the registries that do exist. A client reading
        // `unknown variant \`go\`` has to go and find the list; one reading
        // `expected one of npm, crates, pypi` has the next call in front of
        // it, and this is the message `-32602` carries.
        // `Cow` rather than `&str`: a tool's arguments arrive as a parsed
        // `serde_json::Value`, which owns its strings and has nothing to
        // borrow from.
        let id = Cow::<str>::deserialize(deserializer)?;
        Self::from_id(&id).ok_or_else(|| {
            let known: Vec<&str> = Self::ALL.iter().map(|registry| registry.id()).collect();
            de::Error::custom(format!(
                "unknown registry `{id}`, expected one of: {}",
                known.join(", ")
            ))
        })
    }
}

/// The `registry` enum every tool schema declares.
///
/// Generated from [`Registry::ALL`], which is what makes #23's "the same
/// enum in every tool" true by construction rather than by review: a tool
/// names this type in its `Args` and gets the current list, so a fourth
/// registry cannot reach a model's schema late.
impl JsonSchema for Registry {
    fn schema_name() -> Cow<'static, str> {
        "Registry".into()
    }

    fn schema_id() -> Cow<'static, str> {
        concat!(module_path!(), "::Registry").into()
    }

    /// Inline, rather than a `$ref` into `$defs`. The values are three short
    /// strings and the reader is a model deciding what to pass: a reference
    /// it has to resolve to learn what `registry` accepts is a worse schema
    /// than one that says so where the field is.
    fn inline_schema() -> bool {
        true
    }

    fn json_schema(_generator: &mut SchemaGenerator) -> Schema {
        json_schema!({
            "type": "string",
            "enum": Registry::ALL.map(|registry| registry.id()),
            "description": "The registry that publishes the package.",
        })
    }
}

/// What a version has to be, on every registry.
///
/// Not per-registry, and read in the same breath as a name rule: an agent
/// given [`Registry::name_rule`] and left to guess whether a range works will
/// try one, and a range that resolved to something would be this server
/// picking a version on a user's behalf.
pub const VERSION_RULE: &str = "A version is one published version, spelled the way the \
                                registry spells it: not a range, not a tag, and nothing \
                                normalised — `v4.0.0` and `4.0.0` are different versions.";

/// Where a package's versions are listed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VersionSource {
    /// The document to fetch.
    pub url: String,
}

/// One published version of a package, and when it was published.
///
/// The date is here because it is what the order is computed from — see
/// [`Registry::read_versions`] — rather than because a caller asked for it.
/// It is the string the registry wrote, not a parsed instant: this crate has
/// no calendar in it and does not need one to put releases in order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Version {
    /// The version as the registry spells it.
    pub version: String,
    /// When the registry says it was published, where it says.
    ///
    /// `None` is a real answer and not a gap in this crate: deps.dev leaves
    /// the date off some versions — one of `requests`' 161 and thirty-seven
    /// of `numpy`'s 171 — and those are published releases. Dropping them
    /// would answer "what versions are there" with a list missing a fifth of
    /// them, so they are kept and sorted last.
    pub published_at: Option<String>,
    /// Whether it is a preview rather than a release.
    pub prerelease: bool,
}

/// Where a search for a package is answered, and what to ask it for.
///
/// The two travel together because one of the three needs both: the same URL
/// serves PyPI's index as a web page or as PEP 691's JSON depending on what
/// the request says it accepts. A caller told only where to go would be
/// handed HTML and read no hits out of it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchSource {
    /// The document to fetch, query and limit included where the source
    /// takes them.
    pub url: String,
    /// What the request says it accepts.
    pub accept: &'static str,
    /// Whether this source answers every query with the same document.
    ///
    /// True for a source that is an index rather than a reply, which is
    /// PyPI's and nobody else's: its URL carries no query, so one fetch
    /// serves every search made against it. What that buys is the difference
    /// between holding one document and caching results — an answer that
    /// carried the query in its URL would be a per-query cache, with a
    /// staleness nobody asked for and no bound on what it holds.
    pub whole_index: bool,
}

/// One package a search found.
///
/// Two of the three are optional because the sources differ in what they
/// carry rather than because a registry sometimes forgets: npm and crates.io
/// answer with a version and a summary, and PyPI's index answers with a name
/// and nothing else. Absent therefore means the source does not say, and
/// never that the package has published nothing.
///
/// Not the shape a model reads. That is `search_packages`'s own `Hit`, for
/// the reason [`Version`] is not `list_package_versions`'s: a schema a model
/// reads is written where the tool is, so this module stays the one that
/// knows what a registry is rather than also being the one that talks to a
/// model.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hit {
    /// The package name, as the source spells it.
    pub name: String,

    /// The version the registry would install for a caller that named none,
    /// where the source carries one.
    pub version: Option<String>,

    /// What the package says it is, in the registry's own words, where the
    /// source carries it.
    pub description: Option<String>,
}

/// npm's search answer, cut to the fields a [`Hit`] carries.
///
/// A type of this module's rather than the engine's: the engine builds
/// archive URLs and knows nothing about search, and a `serde_json::Value`
/// walked by hand here would be the same fields with the spelling mistakes
/// left to run time.
#[derive(Deserialize)]
struct NpmSearch {
    objects: Vec<NpmObject>,
}

#[derive(Deserialize)]
struct NpmObject {
    package: NpmPackage,
}

#[derive(Deserialize)]
struct NpmPackage {
    name: String,
    version: Option<String>,
    description: Option<String>,
}

/// crates.io's search answer, cut to the fields a [`Hit`] carries.
#[derive(Deserialize)]
struct CratesSearch {
    crates: Vec<CratesCrate>,
}

#[derive(Deserialize)]
struct CratesCrate {
    name: String,
    default_version: Option<String>,
    description: Option<String>,
}

/// PyPI's index: every project it publishes, and for each of them a name.
///
/// PEP 691's JSON form. The `_last-serial` each project carries is not read
/// — it says when a project last changed, which is not a question a search
/// asks.
#[derive(Deserialize)]
struct PyPiIndex {
    projects: Vec<PyPiProject>,
}

#[derive(Deserialize)]
struct PyPiProject {
    name: String,
}

/// How well a name answers a query, best first.
///
/// Three degrees and no score: a number would invite arithmetic on it, and
/// what this actually knows about a name is which of three things it is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Rank {
    /// The query is the name.
    Exact,
    /// The name starts with the query.
    Prefix,
    /// The name has the query somewhere inside it.
    Contains,
}

/// How well `name` answers `query`, or nothing if it does not.
///
/// `query` arrives already lower-cased, because it is the same query for
/// every one of nine hundred thousand names and lowering it once is the
/// difference between a scan and a scan that allocates. Case is ignored
/// because a half-remembered name is what a search is for; the name is
/// answered with as the index spells it either way.
fn rank(name: &str, query: &str) -> Option<Rank> {
    let name = name.to_lowercase();
    if name == query {
        Some(Rank::Exact)
    } else if name.starts_with(query) {
        Some(Rank::Prefix)
    } else if name.contains(query) {
        Some(Rank::Contains)
    } else {
        None
    }
}

/// Whether a PEP 440 version is a preview rather than a release.
///
/// Read rather than parsed: what is wanted is one bit, and a parser for the
/// whole grammar — epochs, post-releases, local versions, the four spellings
/// of every separator — is a dependency and a surface for a question this
/// small. So the release segment is skipped and what follows it is looked at.
///
/// The markers are PEP 440's, `alpha`, `beta`, `c`, `pre` and `preview`
/// included because the specification normalises those to `a`, `b` and `rc`
/// rather than rejecting them. A digit has to follow, so `1.0build3` is not
/// read as a beta.
///
/// A post-release is deliberately not one of them: `1.0.post1` is a
/// re-release of `1.0`, not a preview of something later, and an agent told
/// to avoid it would be avoiding the newest thing there is.
fn pep440_prerelease(version: &str) -> bool {
    /// What PEP 440 spells a prerelease with, before normalisation.
    const MARKERS: [&str; 8] = ["a", "b", "c", "rc", "alpha", "beta", "pre", "preview"];

    let version = version.to_ascii_lowercase();

    // An epoch is `N!` in front of everything, and says nothing about this.
    let version = version.rsplit('!').next().unwrap_or_default();

    // The release segment — `1.0.2` — and then whichever of the four
    // separators the publisher used, or none, which is also allowed.
    let tail = version.trim_start_matches(|c: char| c.is_ascii_digit() || c == '.');
    let tail = tail.trim_start_matches(['.', '-', '_']);

    // A development release sorts before every other form of the same
    // version, including its own alphas, so it is a preview wherever it sits:
    // `1.0.post1.dev2` is a preview of that post-release.
    if tail.contains("dev") {
        return true;
    }

    MARKERS.iter().any(|marker| {
        tail.strip_prefix(marker).is_some_and(|after| {
            after.is_empty() || after.starts_with(|c: char| c.is_ascii_digit())
        })
    })
}

/// A package name or a query as one path segment or one parameter value.
///
/// Everything outside RFC 3986's unreserved set is escaped, which is more
/// than a package name usually needs and exactly what a scoped npm name does:
/// `@types/node` is one name, and its `/` is not a path separator. Nothing is
/// lower-cased or normalised on the way through — that would be this server
/// deciding a package is a different package.
fn escape(text: &str) -> String {
    let mut escaped = String::with_capacity(text.len());
    for byte in text.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                escaped.push(char::from(byte))
            }
            other => escaped.push_str(&format!("%{other:02X}")),
        }
    }
    escaped
}

/// Every host this server may make a request to.
///
/// The union over the registries there are, which is what makes #10's
/// outbound allowlist a consequence of this module: a registry added to
/// [`Registry::ALL`] brings its hosts with it, and a host no registry names
/// is not reachable however it got into an argument.
pub fn allowed_hosts() -> BTreeSet<String> {
    hosts_of(&Registry::ALL)
}

/// The hosts `registries` between them may be reached at.
///
/// Public so that the derivation can be exercised over a list that is not
/// [`Registry::ALL`] — which is how "adding a registry grows the allowlist"
/// is a test rather than a hope.
pub fn hosts_of(registries: &[Registry]) -> BTreeSet<String> {
    registries
        .iter()
        .flat_map(|registry| registry.hosts())
        .collect()
}

/// Whether this server may fetch `url`.
///
/// The check is the whole origin and not just a substring, because every way
/// of getting a request to the wrong place looks like the right host to a
/// `contains`: a suffix (`registry.npmjs.org.example`), a path
/// (`example/registry.npmjs.org`), userinfo
/// (`registry.npmjs.org@example`). So the authority is taken exactly — no
/// userinfo, no port — and compared against [`allowed_hosts`].
///
/// `https` only. A registry that answered over plain HTTP would be one whose
/// archive anyone on the path can replace, and none of these three needs it.
pub fn allows(url: &str) -> bool {
    match host_of(url) {
        Some(host) => allowed_hosts()
            .iter()
            .any(|allowed| allowed.eq_ignore_ascii_case(host)),
        None => false,
    }
}

/// The host `url` addresses, if it addresses one this server could reach.
///
/// Deliberately strict rather than a URL parser: anything with userinfo or a
/// port in it is refused outright instead of being reduced to the host it
/// claims, because this crate has no reason to build such a URL and a
/// registry has no reason to serve one.
fn host_of(url: &str) -> Option<&str> {
    let authority = url.strip_prefix("https://")?;
    let authority = authority
        .split(['/', '?', '#'])
        .next()
        .filter(|authority| !authority.is_empty())?;

    if authority.contains(['@', ':']) {
        return None;
    }
    Some(authority)
}
