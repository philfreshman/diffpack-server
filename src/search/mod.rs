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
//! same reasons: one seam, two adapters, and [`crate::document`] under all
//! three so there is one HTTP client, one host check and one fixture reader
//! for the registries rather than one of each per seam.
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
//! the rest, and it is untouched: what these three share is an
//! implementation, not an interface. [ADR
//! 0015](../docs/adr/0015-one-implementation-beneath-three-registry-seams.md)
//! is where that distinction is argued.
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
//! The one thing a warm instance holds is PyPI's *document* — for
//! [`FRESH_FOR`], which is this module's number — and every query is matched
//! against it afresh.

use std::path::PathBuf;
use std::time::Duration;

use crate::document::{About, Document};
use crate::error::Failure;
use crate::registry::{Hit, Registry};

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

/// How long an index this server has already fetched is treated as current.
///
/// PyPI serves its index with `cache-control: max-age=600`, so ten minutes is
/// the registry's own answer to the question rather than a number chosen
/// here. It is written down rather than read back off the response: nothing
/// in this crate parses a header, so a `max-age` PyPI changed is a change to
/// this line and not one that arrives on its own.
///
/// A package published inside that window is missing from a search made
/// inside it, which is the trade the source costs: the alternative is tens of
/// megabytes on every call.
///
/// It is this module's constant and goes to [`crate::document`] as an
/// argument. Which source is worth holding, and for how long, is a fact about
/// what a registry publishes; the holding itself has to sit under the cap,
/// and the cap is that module's.
const FRESH_FOR: Duration = Duration::from_secs(600);

/// What a source that is not answering answers with, where it answers at all.
///
/// The number a fixture stands in with, so that the refusal the suite drives
/// is the one a real outage produces rather than one shaped like it. Not a
/// `404` the way the other two sets' `null` is: nothing in a search URL names
/// a package, so a search source answering as though there were is a source
/// that has moved rather than a thing that is missing.
const SOURCE_DOWN: u16 = 503;

/// Where the packages a registry has are looked for.
#[derive(Debug)]
pub struct Search {
    documents: Document,
}

impl Search {
    /// The registries themselves, which is what production asks.
    pub fn live() -> Self {
        Self {
            documents: Document::live(SIZE_LIMIT),
        }
    }

    /// Answers read from `dir` rather than from a registry.
    ///
    /// `dir` holds an `index.json` mapping a URL to the file beside it that
    /// stands in for what that URL serves — the archive fixtures'
    /// convention, for the archive fixtures' reason.
    pub fn fixture(dir: impl Into<PathBuf>) -> Self {
        Self {
            documents: Document::fixture(dir, "reading the search fixtures", SIZE_LIMIT),
        }
    }

    /// The same source, refusing an answer over `limit` bytes.
    pub fn with_limit(self, limit: u64) -> Self {
        Self {
            documents: self.documents.with_limit(limit),
        }
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
        let cap = self.documents.limit();
        let source = registry.search(query, limit);

        let body = self
            .documents
            .body(
                &source.url,
                &About {
                    registry,
                    accept: Some(source.accept),
                    // The source that is a whole index is the one worth
                    // decoding on the way in, and it is the same fact that
                    // makes it worth holding: a source that answers every
                    // query with one document is the one whose document is
                    // large. See `fetch::About`.
                    compressed: source.whole_index,
                    // And it is the one worth holding between invocations,
                    // for the same fact a third time: a document that answers
                    // every query is a document a second search would fetch
                    // again for nothing.
                    remember_for: source.whole_index.then_some(FRESH_FOR),
                    nothing_there: SOURCE_DOWN,
                    // A URL this crate built, checked anyway — so that the
                    // rule is "every outbound request is checked" rather than
                    // "every outbound request that somebody thought about".
                    // Nothing a model can act on: a search URL off the
                    // allowlist is two files in this repository disagreeing.
                    blocked: &|| Failure::Internal {
                        doing: "asking a registry which packages it has",
                    },
                    // A search URL names no package, so there is nothing for
                    // one of these to be missing: a source answering as
                    // though there were is a source that has moved.
                    missing: &|status| unavailable(registry, status),
                    too_large: &|bytes| too_large(registry, bytes, cap),
                },
            )
            .await?;

        let body = std::str::from_utf8(&body)
            .map_err(|_| unreadable(registry, "what the registry served is not text"))?;

        registry.read_hits(body, query, limit).ok_or_else(|| {
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
