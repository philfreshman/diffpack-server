//! What a registry publishes, asked of the registry.
//!
//! What this module knows is what a status means when the thing being asked
//! for is a catalogue rather than an archive — which is not the same mapping.
//! A `404` here is not "no such package": nothing in the URL names a package,
//! so a search source answering `404` is a source that has moved or broken,
//! and the remedy is to try again or to search elsewhere rather than to check
//! a spelling.

use std::sync::{Arc, OnceLock, RwLock};
use std::time::{Duration, Instant};

use reqwest::{Response, StatusCode};

use crate::error::Failure;
use crate::http;
use crate::registry::{Registry, SearchSource};

/// How long an index this server has already fetched is treated as current.
///
/// PyPI serves its index with `cache-control: max-age=600`, so ten minutes is
/// the registry's own answer to the question rather than a number chosen
/// here. A package published inside that window is missing from a search made
/// inside it, which is the trade the source costs: the alternative is 44 MB
/// on every call.
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
    ) -> Result<Arc<str>, Failure> {
        if source.whole_index {
            if let Some(body) = remembered(&source.url) {
                return Ok(body);
            }
        }

        let body = self.fetch(source, limit, registry).await?;

        if source.whole_index {
            remember(&source.url, &body);
        }
        Ok(body)
    }

    /// The request itself.
    async fn fetch(
        &self,
        source: &SearchSource,
        limit: u64,
        registry: Registry,
    ) -> Result<Arc<str>, Failure> {
        let asked = http::get(&source.url, source.accept, registry).await?;
        let response = success(asked, registry)?;

        // An answer this server will not hold, and one that is not text, are
        // the same thing to a caller: the source answered and what it sent
        // is unusable. The status it answered *with* is carried rather than
        // invented, so the message names something that really happened.
        let status = response.status().as_u16();
        let unusable = || super::unavailable(registry, status);

        let bytes = http::read_within(response, limit, registry.name(), |_| unusable()).await?;
        String::from_utf8(bytes)
            .map(Arc::from)
            .map_err(|_| unusable())
    }
}

/// What this instance last fetched from `url`, while it is still current.
///
/// A serverless function is built once and invoked many times, and the one
/// source that is a whole document is the one worth remembering between
/// invocations: 44 MB fetched per search is a tool an agent stops using.
/// What is held is the document, not an answer — every query is still matched
/// against it — so this is not a result cache and nothing here reaches the
/// blob store, which is the budget #19 says a search must not spend.
///
/// The lock is poisoned only by a panic while a writer held it, which cannot
/// happen here: the guarded value is replaced by an assignment. A poisoned
/// lock is treated as an empty memo rather than a failure, because a slow
/// search is a better answer than a broken one.
fn remembered(url: &str) -> Option<Arc<str>> {
    let memo = memo().read().ok()?;
    let held = memo.as_ref()?;

    (held.url == url && held.fetched.elapsed() < FRESH_FOR).then(|| Arc::clone(&held.body))
}

/// Hold `body` as what `url` serves, until [`FRESH_FOR`] has passed.
///
/// One document at a time. Two would be a cache with an eviction policy to
/// choose, and there is one source in this shape.
fn remember(url: &str, body: &Arc<str>) {
    if let Ok(mut memo) = memo().write() {
        *memo = Some(Index {
            url: url.to_owned(),
            fetched: Instant::now(),
            body: Arc::clone(body),
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
    body: Arc<str>,
}

/// The response, or the failure its status names.
///
/// Every arm is transient or nearly so, which is the difference from the
/// archive path: there is no argument here that could have been wrong, so
/// there is no remedy that involves the caller changing one. A search that
/// fails says "try again", and says which registry it was.
fn success(response: Response, registry: Registry) -> Result<Response, Failure> {
    let status = response.status();
    if status.is_success() {
        return Ok(response);
    }

    Err(match status {
        StatusCode::TOO_MANY_REQUESTS => Failure::RateLimited {
            registry: registry.name().to_owned(),
            retry_after: http::retry_after(&response),
        },

        other => super::unavailable(registry, other.as_u16()),
    })
}
