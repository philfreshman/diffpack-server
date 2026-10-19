use rmcp::model::{CacheScope, ReadResourceResult, Resource, ResourceContents};
use serde::Serialize;

use crate::error::Failure;
use crate::registry::{ArchiveSource, Registry, VERSION_RULE};

pub const URI: &str = "diffpack://registries";

pub const RESOURCE_NAME: &str = "registries";

pub fn resource() -> Resource {
    Resource::new(URI, RESOURCE_NAME)
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

const TTL_MS: u64 = 60 * 60 * 1000;

pub fn read() -> Result<ReadResourceResult, Failure> {
    let catalogue = Catalogue {
        version_rule: VERSION_RULE,
        registries: Registry::ALL.into_iter().map(Described::of).collect(),
    };

    let document = serde_json::to_string_pretty(&catalogue).map_err(|_| Failure::Internal {
        doing: "describing the registries",
    })?;

    let contents = ResourceContents::text(document, URI).with_mime_type("application/json");

    Ok(ReadResourceResult::new(vec![contents])
        .with_ttl_ms(TTL_MS)
        .with_cache_scope(CacheScope::Public))
}

#[derive(Serialize)]
struct Catalogue {
    version_rule: &'static str,

    registries: Vec<Described>,
}

#[derive(Serialize)]
struct Described {
    id: &'static str,

    name: &'static str,

    name_rule: &'static str,

    archive: Archive,

    versions: Versions,

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

#[derive(Serialize)]
struct Archive {
    kind: Kind,

    url_pattern: String,
}

#[derive(Serialize)]
#[serde(rename_all = "snake_case")]
enum Kind {
    Archive,
    Listing,
}

impl Archive {
    fn of(registry: Registry) -> Self {
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

#[derive(Serialize)]
struct Versions {
    url_pattern: String,
}

impl Versions {
    fn of(registry: Registry) -> Self {
        Self {
            url_pattern: named(&registry.versions(&package()).url),
        }
    }
}

#[derive(Serialize)]
struct Search {
    url_pattern: String,

    accept: &'static str,

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

const SCOPE: &str = "SCOPEPLACEHOLDER";
const NAME: &str = "NAMEPLACEHOLDER";
const VERSION: &str = "VERSIONPLACEHOLDER";
const QUERY: &str = "QUERYPLACEHOLDER";

fn package() -> String {
    format!("{SCOPE}/{NAME}")
}

fn named(url: &str) -> String {
    url.replace(&format!("{SCOPE}%2F{NAME}"), "{package}")
        .replace(&format!("{SCOPE}/{NAME}"), "{package}")
        .replace(NAME, "{package-without-scope}")
        .replace(VERSION, "{version}")
        .replace(QUERY, "{query}")
}

const ONE: u32 = 1;
const TWO: u32 = 2;

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
