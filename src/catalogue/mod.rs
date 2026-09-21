//! What a registry says it publishes, without downloading any of it.
//!
//! One interface — [`Catalogue::search`] — and everything asking it costs on
//! the far side: where the source is, the request, the size cap, and reading
//! an answer whose shape is a different shape per registry. A tool asks for
//! [`Hit`]s and is given them or a [`Failure`]; that PyPI's answer was the
//! whole index and npm's was a ranked list is not a tool's to know.
//!
//! This is the archive seam's sibling and is deliberately not the archive
//! seam. The two answer different questions — *what is in this version* and
//! *what does this registry have* — and the second needs no version, produces
//! no [`FileMap`](crate::archive::FileMap), and is asked by tools that never
//! download anything.
//!
//! # Two adapters
//!
//! [`Catalogue::live`] asks the registries. [`Catalogue::fixture`] reads
//! answers from a directory on disk, keyed by URL exactly as the archive
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

mod fixture;
mod live;

use std::path::PathBuf;

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
/// Distinct from the archive's own limit, and smaller on purpose: an 80 MB
/// crate is an ordinary thing to diff, and an 80 MB answer to "which
/// packages are called something like this" is not an ordinary anything.
pub const SIZE_LIMIT: u64 = 64 * 1024 * 1024;

/// Where what a registry publishes is read from.
#[derive(Debug)]
pub struct Catalogue {
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

impl Catalogue {
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
    /// stands in for what that URL serves — the archive fixtures' convention,
    /// for the archive fixtures' reason.
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
    pub async fn search(
        &self,
        registry: Registry,
        query: &str,
        limit: u32,
    ) -> Result<Vec<Hit>, Failure> {
        let source = registry.search(query, limit);

        // The same check the archive path makes, and not because this URL
        // could plausibly be wrong: it is built by `registry` and is allowed
        // by construction. It is here so that the rule is "every outbound
        // request is checked" rather than "every outbound request that
        // somebody thought about".
        if !registry::allows(&source.url) {
            return Err(Failure::Internal {
                doing: "asking a registry for what it publishes",
            });
        }

        let body = match &self.source {
            Source::Live(live) => live.body(&source, self.limit, registry).await?,
            Source::Fixture(fixture) => fixture.body(&source.url, registry)?,
        };

        registry
            .read_hits(&body, query, limit)
            .ok_or(Failure::Internal {
                doing: "reading what a registry publishes",
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
pub(super) fn unavailable(registry: Registry, status: u16) -> Failure {
    Failure::Unavailable {
        registry: registry.name().to_owned(),
        status,
    }
}
