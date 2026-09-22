//! `diffpack://registries` — the registries this server knows, described
//! once.
//!
//! Everything here is read out of [`crate::registry`] rather than written
//! down: the identifiers, the names, where each registry's archives, versions
//! and searches come from, and the name rules an agent would otherwise guess
//! at. See ADR 0004 — a hand-written catalogue beside that module is a fifth
//! copy of what four other places already know, and the first URL that
//! changes leaves it telling an agent something untrue.
//!
//! # Where the patterns come from
//!
//! A URL pattern is not a string in this file. The module is asked for a URL
//! with placeholders standing in for the package, the version and the query,
//! and what comes back *is* the pattern — so a registry whose address changes
//! changes this resource in the same commit, and `tests/resources.rs` holds
//! the two against each other besides.
//!
//! The placeholders are spelled in the unreserved characters of RFC 3986, so
//! `registry`'s own escaping leaves them alone and each comes back out of a
//! URL as the one substring it went in as. The limit cannot be one — it is a
//! number — so it is found by asking twice and naming whatever moved. Which
//! parameter carries it is the registry's own business (`size` on npm,
//! `per_page` on crates.io, and PyPI's index takes none at all), and a
//! `match` here would be this module's copy of that fact.

use rmcp::model::{Resource, ResourceContents};
use serde::Serialize;

use crate::registry::{ArchiveSource, Registry, VERSION_RULE};

/// The URI a client reads this at.
pub const URI: &str = "diffpack://registries";

/// The catalogue, as `resources/list` shows it.
pub fn resource() -> Resource {
    Resource::new(URI, "registries")
        .with_title("Package registries")
        .with_description(
            "The registries this server knows — npm, crates.io and PyPI — with the \
             identifier each is named by in a tool argument, where a version's archive, \
             a package's versions and a search come from, and how each registry spells a \
             package name. Read this once instead of guessing at a scoped npm name or at \
             whether a version range resolves.",
        )
        .with_mime_type("application/json")
}

/// The catalogue itself.
pub fn read() -> ResourceContents {
    let catalogue = Catalogue {
        version_rule: VERSION_RULE,
        registries: Registry::ALL.into_iter().map(Described::of).collect(),
    };

    // A document this server built out of its own types, so a failure to
    // serialise it is impossible rather than handled: every field is a
    // string, a bool or a list of those.
    ResourceContents::text(
        serde_json::to_string_pretty(&catalogue).unwrap_or_default(),
        URI,
    )
    .with_mime_type("application/json")
}

/// What a reader is handed.
#[derive(Serialize)]
struct Catalogue {
    /// What a version has to be, on every registry.
    version_rule: &'static str,

    /// The registries, in the order [`Registry::ALL`] lists them.
    registries: Vec<Described>,
}

/// One registry, as this resource describes it.
#[derive(Serialize)]
struct Described {
    /// The identifier a tool argument spells.
    id: &'static str,

    /// The name the registry calls itself, which is what a message says.
    name: &'static str,

    /// What this registry's spelling of a package name costs a caller.
    name_rule: &'static str,

    /// Where a version's archive comes from.
    archive: Archive,

    /// Where a package's published versions are listed.
    versions: Versions,

    /// Where a search for a package is answered.
    search: Search,
}

impl Described {
    fn of(registry: Registry) -> Self {
        Self {
            id: registry.id(),
            name: registry.name(),
            name_rule: registry.name_rule(),
            archive: Archive::of(registry),
            versions: Versions::of(registry),
            search: Search::of(registry),
        }
    }
}

/// Where a version's archive is, and whether that is the archive itself.
#[derive(Serialize)]
struct Archive {
    /// Whether the URL is the archive or a document naming it.
    kind: Kind,

    /// The URL, with the fields to fill in named.
    url_pattern: String,
}

/// The two things an archive URL can be.
///
/// A registry's own fact and not a caller's: npm and crates.io publish at a
/// path anyone can construct, and PyPI lists a version's files in its own
/// metadata. Which of the two it is decides whether one request is enough.
#[derive(Serialize)]
#[serde(rename_all = "snake_case")]
enum Kind {
    /// The archive itself.
    Archive,
    /// A document listing the version's files, one of which is the archive.
    Listing,
}

impl Archive {
    fn of(registry: Registry) -> Self {
        // Unreachable: the `Err` is the engine refusing an identifier this
        // module handed it, which is two files in this repository
        // disagreeing rather than anything a reader did. A listing at an
        // empty URL is the fail-visible answer and cheaper than a panic
        // inside a request.
        let (kind, url) = match registry.archive(&package(), VERSION) {
            Ok(ArchiveSource::Archive { url }) => (Kind::Archive, url),
            Ok(ArchiveSource::Listing { url }) => (Kind::Listing, url),
            Err(_) => (Kind::Listing, String::new()),
        };

        Self {
            kind,
            url_pattern: named(&url),
        }
    }
}

/// Where a package's versions are listed.
#[derive(Serialize)]
struct Versions {
    /// The URL, with the fields to fill in named.
    url_pattern: String,
}

impl Versions {
    fn of(registry: Registry) -> Self {
        Self {
            url_pattern: named(&registry.versions(&package()).url),
        }
    }
}

/// Where a search is answered, and what to ask it for.
#[derive(Serialize)]
struct Search {
    /// The URL, with the fields to fill in named.
    url_pattern: String,

    /// What the request has to say it accepts.
    accept: &'static str,

    /// Whether this source answers every query with the same document, so
    /// that the query and the limit are applied on this side rather than
    /// sent.
    whole_index: bool,
}

impl Search {
    fn of(registry: Registry) -> Self {
        let source = registry.search(QUERY, ONE);

        Self {
            url_pattern: limit_named(
                &named(&source.url),
                &named(&registry.search(QUERY, TWO).url),
            ),
            accept: source.accept,
            whole_index: source.whole_index,
        }
    }
}

/// The placeholders, and what each stands for once a URL comes back carrying
/// it.
///
/// Every one is spelled in RFC 3986's unreserved characters, so `registry`'s
/// escaping leaves it whole and it comes back out of a URL as the substring
/// it went in as. The scope and the name are probed as one package name with
/// a `/` between them — which is what a scoped npm name is — so that a URL
/// carrying the whole name and a URL carrying the name alone come back
/// distinguishable. `@types/node` is served from
/// `registry.npmjs.org/@types/node/-/node-20.1.0.tgz`, and a pattern with
/// `{package}` in both slots would be wrong about the one kind of name this
/// resource exists to stop an agent guessing at.
const SCOPE: &str = "SCOPEPLACEHOLDER";
const NAME: &str = "NAMEPLACEHOLDER";
const VERSION: &str = "VERSIONPLACEHOLDER";
const QUERY: &str = "QUERYPLACEHOLDER";

/// The probe package name: a scoped one, so both spellings are visible.
fn package() -> String {
    format!("{SCOPE}/{NAME}")
}

/// `url` with each placeholder replaced by the field it stands for.
///
/// The whole name first and in both its spellings — a `/` survives into a
/// path unescaped and is percent-encoded into a segment — so that what is
/// left of `NAME` afterwards is only the slot a registry fills with the name
/// alone.
fn named(url: &str) -> String {
    url.replace(&format!("{SCOPE}%2F{NAME}"), "{package}")
        .replace(&format!("{SCOPE}/{NAME}"), "{package}")
        .replace(NAME, "{package-without-scope}")
        .replace(VERSION, "{version}")
        .replace(QUERY, "{query}")
}

/// The two limits the search URL is probed with.
///
/// Both below every source's own maximum, so each reaches the URL as it was
/// asked for rather than narrowed to the same number twice.
const ONE: u32 = 1;
const TWO: u32 = 2;

/// `one` with whatever differs from `two` named as the limit.
///
/// Which parameter carries it is the registry's own business — `size` on
/// npm, `per_page` on crates.io, and PyPI's index takes none at all — so it
/// is found by asking twice and naming what moved. A `match` here would be
/// this module's second copy of a fact `registry` already holds, which is the
/// whole thing ADR 0004 is about. Two URLs that do not differ is the source
/// that takes no limit, and nothing is named.
///
/// Byte indices, which are character boundaries here because every URL this
/// module builds is percent-encoded ASCII.
fn limit_named(one: &str, two: &str) -> String {
    if one == two {
        return one.to_owned();
    }

    let head = one
        .bytes()
        .zip(two.bytes())
        .take_while(|(a, b)| a == b)
        .count();
    let tail = one
        .bytes()
        .rev()
        .zip(two.bytes().rev())
        .take_while(|(a, b)| a == b)
        .count()
        .min(one.len() - head);

    format!("{}{{limit}}{}", &one[..head], &one[one.len() - tail..])
}
