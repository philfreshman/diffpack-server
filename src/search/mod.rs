//! Which packages a registry has that answer to a query.
//!
//! One interface — [`Search::hits`] — and everything asking a registry that
//! question costs on the far side: where the source is, what the request has
//! to say it accepts, the size cap, and reading an answer whose shape is a
//! different shape per registry. A tool asks for [`Hit`]s and is given them
//! or a [`Failure`]; that PyPI's answer was its whole index and npm's was a
//! ranked reply is not a tool's to know.
//!
//! The same shape as [`crate::archive`] and [`crate::catalogue`] and for the
//! same reasons: one seam, two adapters, and [`crate::fetch`] underneath the
//! live one so there is one HTTP client for the registries rather than one
//! per seam.
//!
//! # Why this is not the catalogue seam
//!
//! [`crate::catalogue`] answers *what has this package released*. This
//! answers *which packages are there*. They share a registry and nothing
//! else: a catalogue is asked about a package that the caller can already
//! name, a search is what a caller uses when it cannot; a catalogue's `404`
//! is a package that does not exist, and a search source's is the source
//! itself having moved. They also cost differently — one document per
//! package against, for PyPI, the index of every project there is — and the
//! policy that difference needs lives here rather than in a module whose
//! header says it is about one package's releases. [ADR
//! 0011](../docs/adr/0011-what-a-registry-publishes-is-its-own-seam.md) has
//! the rest.
//!
//! # Two adapters
//!
//! [`Search::live`] asks the registries. [`Search::fixture`] reads answers
//! from a directory on disk, keyed by URL exactly as the archive and version
//! fixtures are, which is what lets the suite assert what a search returns
//! with no network — and what makes a fixture test a test of *where* each
//! registry is asked, since a search that built a URL of its own finds
//! nothing there.
//!
//! The fixture index has a third answer beside "here is the body" and "this
//! URL is not in the set": `null`, meaning the source is not answering. It is
//! how the offline suite reaches the path a source being down takes, and the
//! refusal it produces is built by [`unavailable`] — the live adapter's own
//! constructor — so the two cannot disagree about what a quiet source reads
//! like.
//!
//! # Nothing here is a cached result
//!
//! What a registry has moves whenever somebody publishes, and the 256 MB
//! budget belongs to diff results, so no answer is written to the blob store.
//! The one thing a warm instance holds is PyPI's *document* — see
//! [`live`] — and every query is matched against it afresh.

mod fixture;
mod live;

use std::path::PathBuf;
use std::sync::Arc;

use crate::error::Failure;
use crate::registry::{self, Hit, Registry};

/// The most one search answer may weigh before this server refuses it unread.
///
/// Sized against the one source that is a whole document rather than a
/// reply: PyPI's index was 44 MB when #19 chose it, and it grows with every
/// project published. The cap is what leaves that room to grow for years
/// while still being a cap — a source that answered with a gigabyte would be
/// filling this function's memory, whatever it called itself.
///
/// Distinct from [`crate::archive::SIZE_LIMIT`], and smaller on purpose: an
/// 80 MB crate is an ordinary thing to diff, and an 80 MB answer to "which
/// packages are called something like this" is not an ordinary anything.
pub const SIZE_LIMIT: u64 = 64 * 1024 * 1024;

/// Where the packages a registry has are looked for.
#[derive(Debug)]
pub struct Search {
    source: Source,
    /// The most an answer may weigh before this server refuses it. A field
    /// rather than the constant read at the point of use, so the refusal can
    /// be exercised with a real answer and a small limit.
    limit: u64,
}

/// The two adapters, as a variant each rather than a trait, for the reason
/// [`crate::archive`] gives: neither can arrive from outside this crate.
#[derive(Debug)]
enum Source {
    Live(live::Live),
    Fixture(fixture::Fixture),
}

impl Search {
    /// The registries themselves, which is what production asks.
    pub fn live() -> Self {
        Self {
            source: Source::Live(live::Live::new()),
            limit: SIZE_LIMIT,
        }
    }

    /// Answers read from `dir` rather than from a registry.
    ///
    /// `dir` holds an `index.json` mapping a URL to the file beside it that
    /// stands in for what that URL serves — the archive fixtures'
    /// convention, for the archive fixtures' reason.
    pub fn fixture(dir: impl Into<PathBuf>) -> Self {
        Self {
            source: Source::Fixture(fixture::Fixture::new(dir.into())),
            limit: SIZE_LIMIT,
        }
    }

    /// The same source, refusing an answer over `limit` bytes.
    pub fn with_limit(self, limit: u64) -> Self {
        Self { limit, ..self }
    }

    /// The packages on `registry` that answer to `query`, at most `limit` of
    /// them.
    ///
    /// Both arguments go to [`crate::registry`] twice: once to build the
    /// source — npm and crates.io take a query and a size, PyPI takes
    /// neither — and once to read what came back, which is where a source
    /// that could not be asked has its query applied instead.
    pub async fn hits(
        &self,
        registry: Registry,
        query: &str,
        limit: u32,
    ) -> Result<Vec<Hit>, Failure> {
        let source = registry.search(query, limit);

        // The same check the archive and catalogue paths make, and not
        // because this URL could plausibly be wrong: it is built by
        // `registry` and is allowed by construction. It is here so that the
        // rule is "every outbound request is checked" rather than "every
        // outbound request that somebody thought about".
        if !registry::allows(&source.url) {
            return Err(Failure::Internal {
                doing: "asking a registry which packages it has",
            });
        }

        let body = match &self.source {
            Source::Live(live) => live.body(&source, self.limit, registry).await?,
            Source::Fixture(fixture) => fixture.body(&source.url, registry)?,
        };

        // Weighed here and not only inside the live adapter, for the reason
        // `archive` and `catalogue` weigh theirs here: the cap is a rule
        // about what this server will read rather than about where bytes came
        // from, so a fixture set cannot answer with something the registries
        // would have been refused for.
        let weight = body.len() as u64;
        if weight > self.limit {
            return Err(too_large(registry, weight, self.limit));
        }

        registry.read_hits(&body, query, limit).ok_or_else(|| {
            unreadable(
                registry,
                "the registry answered in a shape this server does not know",
            )
        })
    }
}

/// A search source that did not give this server something it can use, in the
/// words a model reads.
///
/// One constructor for both adapters, so the fixture set cannot answer
/// something the registries would not. It is a tool error rather than a
/// protocol one because the remedy is a model's to choose: try again, or
/// search another registry, or ask for a package by the name it already has.
fn unavailable(registry: Registry, status: u16) -> Failure {
    Failure::Unavailable {
        registry: registry.name().to_owned(),
        status,
    }
}

/// A search answer this server could not read, in the words a model reads.
///
/// [`crate::catalogue`]'s `unreadable` without the package, because there is
/// no package in a search to name: the query was the whole of what was
/// asked, and what went wrong happened to the registry's side of it.
fn unreadable(registry: Registry, reason: &str) -> Failure {
    Failure::UnreadableSearch {
        registry: registry.name().to_owned(),
        reason: reason.to_owned(),
    }
}

/// A search answer this server declined to read, in the words a model reads.
fn too_large(registry: Registry, bytes: u64, limit: u64) -> Failure {
    Failure::SearchTooLarge {
        registry: registry.name().to_owned(),
        bytes,
        limit,
    }
}

/// One search source's answer, as both adapters hand it over.
///
/// Shared rather than a `String` because the one source that is a whole
/// index is held between invocations, and handing a caller its own copy
/// would be the saving spent again on every search.
type Body = Arc<str>;
