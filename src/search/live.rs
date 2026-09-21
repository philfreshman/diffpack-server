//! Which packages a registry has, asked of the registry.
//!
//! The counterpart of [`crate::archive::live`] and [`crate::catalogue::live`],
//! and as thin: the client is [`crate::fetch`]'s, and what is here is the
//! part that is about *searches*.
//!
//! What that part is, is a status mapping that is not the other two's and one
//! policy neither of them needs. A `404` here is not "no such package":
//! nothing in the URL names a package, so a search source answering `404` is
//! a source that has moved or broken, and the remedy is to try again or to
//! search elsewhere rather than to check a spelling.

use std::sync::{OnceLock, RwLock};
use std::time::{Duration, Instant};

use crate::error::Failure;
use crate::fetch::{self, About};
use crate::registry::{Registry, SearchSource};

use super::Body;

/// How long an index this server has already fetched is treated as current.
///
/// PyPI serves its index with `cache-control: max-age=600`, so ten minutes is
/// the registry's own answer to the question rather than a number chosen
/// here. It is written down rather than read back off the response: nothing
/// here parses a header, so a `max-age` PyPI changed is a change to this line
/// and not one that arrives on its own.
///
/// A package published inside that window is missing from a search made
/// inside it, which is the trade the source costs: the alternative is tens of
/// megabytes on every call.
const FRESH_FOR: Duration = Duration::from_secs(600);

/// The adapter that asks.
#[derive(Debug)]
pub struct Live;

impl Live {
    pub fn new() -> Self {
        Self
    }

    /// Whatever `source` serves, as text, refusing anything over `limit`.
    ///
    /// A source that is the whole index is fetched once per warm instance
    /// rather than once per search — see [`remembered`].
    pub async fn body(
        &self,
        source: &SearchSource,
        limit: u64,
        registry: Registry,
    ) -> Result<Body, Failure> {
        if source.whole_index {
            if let Some(body) = remembered(&source.url) {
                return Ok(body);
            }
        }

        let body = fetch(source, limit, registry).await?;

        if source.whole_index {
            remember(&source.url, &body);
        }
        Ok(body)
    }
}

/// The request itself.
async fn fetch(source: &SearchSource, limit: u64, registry: Registry) -> Result<Body, Failure> {
    let bytes = fetch::bytes(
        &source.url,
        limit,
        &About {
            registry,
            accept: Some(source.accept),
            // The source that is a whole index is the one worth decoding on
            // the way in, and it is the same flag because it is the same
            // fact: a source that answers every query with one document is
            // the one whose document is large. See `fetch::About`.
            compressed: source.whole_index,
            // A search URL names no package, so there is nothing for one of
            // these to be missing: a source answering as though there were
            // is a source that has moved.
            missing: &|status| super::unavailable(registry, status),
            too_large: &|bytes| super::too_large(registry, bytes, limit),
        },
    )
    .await?;

    String::from_utf8(bytes)
        .map(Into::into)
        .map_err(|_| super::unreadable(registry, "what the registry served is not text"))
}

/// What this instance last fetched from `url`, while it is still current.
///
/// A serverless function is built once and invoked many times, and the one
/// source that is a whole document is the one worth remembering between
/// invocations: tens of megabytes fetched per search is a tool an agent stops
/// using. What is held is the document, not an answer — every query is still
/// matched against it — so this is not a result cache and nothing here
/// reaches the blob store, which is the budget #19 says a search must not
/// spend.
///
/// The lock is poisoned only by a panic while a writer held it, which cannot
/// happen here: the guarded value is replaced by an assignment. A poisoned
/// lock is treated as an empty memo rather than a failure, because a slow
/// search is a better answer than a broken one.
fn remembered(url: &str) -> Option<Body> {
    let memo = memo().read().ok()?;
    let held = memo.as_ref()?;

    (held.url == url && held.fetched.elapsed() < FRESH_FOR).then(|| Body::clone(&held.body))
}

/// Hold `body` as what `url` serves, until [`FRESH_FOR`] has passed.
///
/// One document at a time. Two would be a cache with an eviction policy to
/// choose, and there is one source in this shape.
fn remember(url: &str, body: &Body) {
    if let Ok(mut memo) = memo().write() {
        *memo = Some(Index {
            url: url.to_owned(),
            fetched: Instant::now(),
            body: Body::clone(body),
        });
    }
}

fn memo() -> &'static RwLock<Option<Index>> {
    static MEMO: OnceLock<RwLock<Option<Index>>> = OnceLock::new();
    MEMO.get_or_init(|| RwLock::new(None))
}

/// One index, and when this instance fetched it.
struct Index {
    url: String,
    fetched: Instant,
    body: Body,
}
