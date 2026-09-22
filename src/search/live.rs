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

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;

    /// The memo makes an index fetched once per *warm* instance. This is the
    /// cold instance, which is the case it says nothing about: the document
    /// is not there yet, and until the first fetch finishes there is nothing
    /// for a second search to find. Eight searches arriving at once on an
    /// instance nobody has searched on is eight fetches of tens of megabytes,
    /// each one held in full.
    ///
    /// So the burst is what this drives, and a counter is what it holds: the
    /// fetch happens once and the seven that waited are answered with what
    /// the first one got.
    ///
    /// The fake waits before it answers, and that is the part that makes the
    /// test mean anything. A fetch that returned without ever yielding would
    /// be finished before the second call was polled, and eight calls in a
    /// row cost one fetch whether or not anything is guarding them — the same
    /// trap as asserting concurrency by doing a thing twice. Waiting on a
    /// socket is what a real fetch does, and `yield_now` is the smallest
    /// honest version of it.
    #[tokio::test]
    async fn a_cold_burst_fetches_the_index_once() {
        // This test's own URL. The memo holds one document at a time, so a
        // URL another test might fetch is one that could replace this one
        // between the fetch and the calls reading it back.
        const URL: &str = "https://example.invalid/simple/a-cold-burst";

        let fetches = AtomicUsize::new(0);
        let fetch = || async {
            tokio::task::yield_now().await;
            fetches.fetch_add(1, Ordering::SeqCst);
            Ok(Body::from("every package there is"))
        };

        let burst = (0..8).map(|_| once_at_a_time(URL, fetch));
        let answers = futures::future::join_all(burst).await;

        assert_eq!(
            fetches.load(Ordering::SeqCst),
            1,
            "eight searches arriving at once should cost one fetch of the index"
        );

        for answer in &answers {
            let body = answer.as_ref().expect("every search in the burst is answered");
            assert_eq!(
                &**body, "every package there is",
                "the ones that waited should be answered with what the first fetched"
            );
        }
    }
}
