//! Cached diff results.
//!
//! [`DiffStore`] is what the rest of the crate sees of the cache — get the
//! entry for a `DiffKey`, put an entry — and everything underneath it is
//! this module's: the Vercel Blob client, the 256 MB budget and the eviction
//! that keeps it (#22). See [ADR
//! 0003](../docs/adr/0003-the-cache-seam-is-a-store.md).
//!
//! # An entry, and why it is the unit
//!
//! One cached result is two blobs under one `diff_id` — `meta.json` and
//! `patches.json` — written together and evicted together. Half an entry is
//! not a cache hit: a caller that could read one and fail on the other would
//! have to decide what to do about it, and the only correct answer is the
//! one this module gives, which is to recompute.
//!
//! # A cache failure is never a diff failure
//!
//! Every way this module can fail ends in a miss. The store being down costs
//! a recomputed diff, which is what the server did before there was a cache
//! at all; a caller that saw the failure would have to decide that for
//! itself, at every call site, and one of them would decide it differently.
//! So [`DiffStore::get`] answers `None` and [`DiffStore::put`] answers
//! nothing, and neither has a `Result` for a caller to handle.

mod blob;

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};

use crate::cache_key::DiffKey;
use crate::engine::DiffFileEntry;

/// One cached diff result: what a caller puts, and what it gets back.
///
/// The key is in it rather than beside it because an entry names the
/// comparison it is of — that is what a reader coming to a blob with nothing
/// but a `diff_id` has to be told, and `diff_id` is a hash that says nothing.
#[derive(Debug, Clone)]
pub struct Entry {
    /// Which comparison this is the result of.
    pub key: DiffKey,

    /// The whole comparison, as the engine arranged it.
    pub tree: DiffFileEntry,
}

/// What `meta.json` holds.
///
/// The key's own fields are flattened in rather than nested under one, so a
/// reader that computed the key itself — #27 does — can check the blob it
/// was handed is about the comparison it asked for, field by field, without
/// knowing this type exists.
///
/// The totals are not here. They are a walk of the tree below, so storing
/// them too would be two places for one number to be written and one of them
/// would eventually be wrong.
#[derive(Debug, Serialize, Deserialize)]
struct Meta {
    #[serde(flatten)]
    key: DiffKey,
    tree: DiffFileEntry,
}

/// The interface the rest of the crate has to cached results.
pub struct DiffStore {
    source: Source,
}

/// Where a store's blobs actually live.
///
/// Variants rather than a trait, for the reason [ADR
/// 0004](../docs/adr/0004-one-registry-module.md) gives: none of them can
/// arrive from outside this crate, so the extensibility a trait buys has no
/// buyer.
enum Source {
    /// The blob store this project owns, which is what production writes to.
    Live(blob::Api),

    /// This process's own memory, which is what the suite writes to.
    Memory(Memory),

    /// No store at all, naming what could not be reached.
    ///
    /// Not a failure mode invented for the suite: it is what a deployment
    /// missing its credentials gets, and the server it produces is the
    /// server that existed before #21 — correct, and slower.
    Unavailable(&'static str),
}

impl DiffStore {
    /// The store this deployment writes to.
    ///
    /// Credentials are read here rather than at the first write, so that a
    /// deployment configured wrongly is one store that knows it is not there
    /// instead of a failure on every call that touches it.
    pub fn live() -> Self {
        let source = match blob::Credentials::from_env() {
            Ok(credentials) => Source::Live(blob::Api::live(credentials)),
            Err(_) => Source::Unavailable("reading the blob store's credentials"),
        };

        Self { source }
    }

    /// The entry for `key`, if this store holds one.
    ///
    /// `None` covers both of the answers a caller can act on identically:
    /// there is nothing cached, and the store could not say. See the module
    /// header.
    pub async fn get(&self, key: &DiffKey) -> Option<Entry> {
        let meta = self.read(&key.meta_path()).await?;
        let meta: Meta = serde_json::from_slice(&meta).ok()?;

        // A blob whose key is not the key that was asked for is a blob under
        // the wrong pathname, which can only be this server having written
        // it there. Recomputing is cheaper than answering about the wrong
        // comparison.
        if &meta.key != key {
            return None;
        }

        Some(Entry {
            key: meta.key,
            tree: meta.tree,
        })
    }

    /// Remember `entry`.
    pub async fn put(&self, entry: Entry) {
        let meta = Meta {
            key: entry.key,
            tree: entry.tree,
        };

        let Ok(bytes) = serde_json::to_vec(&meta) else {
            return;
        };

        self.write(&meta.key.meta_path(), bytes).await;
    }

    /// The bytes at `pathname`, if the store holds any.
    async fn read(&self, pathname: &str) -> Option<Vec<u8>> {
        match &self.source {
            Source::Live(api) => api.read(pathname).await.ok().flatten(),
            Source::Memory(memory) => memory.read(pathname),
            Source::Unavailable(_) => None,
        }
    }

    /// Put `bytes` at `pathname`, or do not.
    async fn write(&self, pathname: &str, bytes: Vec<u8>) {
        match &self.source {
            Source::Live(api) => {
                let _ = api.put(pathname, bytes).await;
            }
            Source::Memory(memory) => memory.write(pathname, bytes),
            Source::Unavailable(_) => {}
        }
    }
}

/// Names the adapter and nothing else.
///
/// A live store holds a bearer token, and a derived `Debug` would put it in
/// any `{:?}` of the [`Ctx`](crate::tools::Ctx) that carries one.
impl std::fmt::Debug for DiffStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let source = match &self.source {
            Source::Live(_) => "live",
            Source::Memory(_) => "memory",
            Source::Unavailable(why) => why,
        };

        f.debug_struct("DiffStore")
            .field("source", &source)
            .finish()
    }
}

/// A store that keeps its blobs in this process, and the handle a test reads
/// them back through.
///
/// The same shape as [`Capture`](crate::log::Capture), and for the same
/// reason: a store whose writes a test cannot see can only be asked whether
/// an answer was cheap, and what the cache promises is more than that — which
/// blobs an entry is, and when each of them was written.
///
/// Blob-shaped rather than entry-shaped on purpose. The pathnames are the
/// contract #27 reads a result back from, so a map keyed by anything else
/// would be a suite that passes while the layout is wrong.
#[derive(Debug, Clone, Default)]
pub struct Memory(Arc<Mutex<BTreeMap<String, Vec<u8>>>>);

impl Memory {
    pub fn new() -> Self {
        Self::default()
    }

    /// The store that writes here, to hand to a [`Ctx`](crate::tools::Ctx).
    pub fn store(&self) -> DiffStore {
        DiffStore {
            source: Source::Memory(self.clone()),
        }
    }

    fn read(&self, pathname: &str) -> Option<Vec<u8>> {
        self.0.lock().ok()?.get(pathname).cloned()
    }

    fn write(&self, pathname: &str, bytes: Vec<u8>) {
        if let Ok(mut blobs) = self.0.lock() {
            blobs.insert(pathname.to_owned(), bytes);
        }
    }
}
