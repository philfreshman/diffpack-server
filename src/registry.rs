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

    /// Where `package`'s versions are listed, and which way that source
    /// lists them.
    ///
    /// The two travel together because #18 promises one order — newest
    /// first, every registry — and only crates.io answers that way already.
    /// A caller told where to ask and left to remember which way the answer
    /// runs is a caller that lists npm backwards.
    ///
    /// The package name is escaped rather than interpolated: a scoped npm
    /// name is one package name, and `@types/node` written into a path
    /// unescaped is a request for a package called `node` inside a scope.
    pub fn versions(self, package: &str) -> VersionSource {
        let escaped = escape(package);
        match self {
            Self::Npm => VersionSource {
                url: format!("https://registry.npmjs.org/{escaped}"),
                order: Order::OldestFirst,
            },
            Self::Crates => VersionSource {
                url: format!("https://crates.io/api/v1/crates/{escaped}"),
                order: Order::NewestFirst,
            },
            // PyPI's own JSON names a package's releases but without the
            // dates that order them, and its simple index is HTML. deps.dev
            // is what the web app already reads for the same reason, so the
            // two agree about what versions a package has.
            Self::PyPi => VersionSource {
                url: format!("https://api.deps.dev/v3/systems/pypi/packages/{escaped}"),
                order: Order::OldestFirst,
            },
        }
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
        let url = match self {
            Self::Npm => {
                format!("https://registry.npmjs.org/-/v1/search?text={escaped}&size={limit}")
            }
            Self::Crates => format!("https://crates.io/api/v1/crates?q={escaped}&per_page={limit}"),
            Self::PyPi => "https://pypi.org/simple/".to_owned(),
        };
        SearchSource { url }
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
    pub fn read_hits(self, body: &str, _query: &str, limit: u32) -> Option<Vec<Hit>> {
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

            Self::PyPi => return None,
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
    /// The order that document lists versions in.
    pub order: Order,
}

/// Where a search for a package is answered.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchSource {
    /// The document to fetch, query and limit included.
    pub url: String,
}

/// One package a search found.
///
/// The fields a model is shown, and they are optional because the sources
/// differ in what they carry rather than because a registry sometimes
/// forgets: npm and crates.io answer with a version and a summary, and
/// PyPI's index answers with a name and nothing else.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, JsonSchema)]
pub struct Hit {
    /// The package name, spelled as the registry spells it. Pass it back
    /// verbatim to any tool that takes a package.
    pub name: String,

    /// The latest version the search source knows of, where it carries one.
    /// Absent is not "no releases" — it is a source that does not say.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,

    /// What the package says it is, in the registry's own words, where the
    /// search source carries it.
    #[serde(skip_serializing_if = "Option::is_none")]
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

/// Which end of a version list the newest release is at.
///
/// Not a detail of parsing: #18 answers newest-first whatever was asked, so
/// this is what a caller reverses by.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Order {
    /// The source lists the newest release first, which is the order a caller
    /// answers in.
    NewestFirst,
    /// The source lists the oldest release first, so a caller reverses it.
    OldestFirst,
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
