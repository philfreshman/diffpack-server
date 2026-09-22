//! Documents from the registries themselves.
//!
//! What is left here after [`crate::fetch`] took the client is two things.
//! One is the translation: a [`super::About`] is `fetch`'s plus the questions
//! a host check and a fixture index ask, and this is where the three that are
//! not `fetch`'s are dropped. The other is the one source that is worth not
//! asking for twice.
//!
//! # A source a warm instance holds
//!
//! A serverless function is built once and invoked many times, and one of the
//! sources this server reads is a whole index rather than a reply to a query:
//! tens of megabytes fetched per search is a tool an agent stops using. What
//! is held is the *document*, not an answer — the query is still matched
//! against it on every call — so this is not a result cache and nothing here
//! reaches the blob store, which is the budget #19 says a search must not
//! spend.
//!
//! Whether a source is like that, and for how long what it served stays
//! current, are both the asking seam's to say and arrive in
//! [`super::About::remember_for`]. What is here is only the holding, and it
//! is here rather than in that seam because of where the cap is: a body
//! handed back from a warm instance is still a body this server is holding,
//! and a seam built `with_limit` has to be able to refuse it. See
//! [`super`]'s header.

use std::sync::{Arc, OnceLock, RwLock};
use std::time::{Duration, Instant};

use crate::error::Failure;
use crate::fetch::{self, About as Fetching};

use super::{About, Body};

/// The adapter that fetches.
#[derive(Debug)]
pub struct Live;

impl Live {
    pub fn new() -> Self {
        Self
    }

    /// What this instance already holds for `url`, where it holds anything
    /// and the caller said a held body would do.
    ///
    /// Separate from [`fetch`](Self::fetch) rather than folded into it so
    /// that [`super::Document`] can tell the two apart: a body that was
    /// streamed has been weighed against the cap on the way in and a body
    /// that was held has not, and which of those happened decides whether it
    /// is weighed again.
    pub fn held(&self, url: &str, about: &About<'_>) -> Option<Body> {
        let fresh_for = about.remember_for?;
        remembered(url, fresh_for).map(Body::Shared)
    }

    /// Whatever `url` serves, refusing anything over `limit`.
    ///
    /// The weighing here is `fetch`'s and is not repeated by the caller: it
    /// refuses a declared length before a byte of the body is read and stops
    /// the running total at the limit, which is a cap applied before the
    /// memory it is guarding has been spent rather than after.
    pub async fn fetch(&self, url: &str, limit: u64, about: &About<'_>) -> Result<Body, Failure> {
        let bytes = fetch::bytes(
            url,
            limit,
            &Fetching {
                registry: about.registry,
                accept: about.accept,
                compressed: about.compressed,
                missing: about.missing,
                too_large: about.too_large,
            },
        )
        .await?;

        // Shared only where something will hold it. Every other body is one
        // call's, and copying it into an `Arc` on the way out would be up to
        // the cap in bytes moved for a pointer nobody clones.
        match about.remember_for {
            Some(_) => {
                let body: Arc<[u8]> = Arc::from(bytes);
                remember(url, &body);
                Ok(Body::Shared(body))
            }
            None => Ok(Body::Owned(bytes)),
        }
    }
}

/// What this instance last fetched from `url`, while it is still current.
///
/// The lock is poisoned only by a panic while a writer held it, which cannot
/// happen here: the guarded value is replaced by an assignment. A poisoned
/// lock is treated as an empty memo rather than a failure, because a slow
/// answer is a better one than a broken one.
fn remembered(url: &str, fresh_for: Duration) -> Option<Arc<[u8]>> {
    let memo = memo().read().ok()?;
    let held = memo.as_ref()?;

    (held.url == url && held.fetched.elapsed() < fresh_for).then(|| Arc::clone(&held.body))
}

/// Hold `body` as what `url` serves, until the caller's window has passed.
///
/// One document at a time. Two would be a cache with an eviction policy to
/// choose, and there is one source in this shape.
fn remember(url: &str, body: &Arc<[u8]>) {
    if let Ok(mut memo) = memo().write() {
        *memo = Some(Held {
            url: url.to_owned(),
            fetched: Instant::now(),
            body: Arc::clone(body),
        });
    }
}

fn memo() -> &'static RwLock<Option<Held>> {
    static MEMO: OnceLock<RwLock<Option<Held>>> = OnceLock::new();
    MEMO.get_or_init(|| RwLock::new(None))
}

/// One document, and when this instance fetched it.
struct Held {
    url: String,
    fetched: Instant,
    body: Arc<[u8]>,
}
