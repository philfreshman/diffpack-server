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
use std::future::Future;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use vercel_runtime::{AppState, LogContext};

use serde::{Deserialize, Serialize};

use crate::cache_key::DiffKey;
use crate::engine::{DiffFileEntry, Patch};
use crate::log::{Note, Sink};

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

    /// The rendered patch for each file that changed, by path.
    ///
    /// Written now because the extraction has already happened: rendering
    /// every changed file at this moment costs almost nothing, where doing it
    /// later costs two archive downloads (#15).
    pub patches: BTreeMap<String, Patch>,
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

    /// Whether this entry was written without its patches.
    ///
    /// The difference between a comparison whose patches were dropped and
    /// one with nothing to patch, which is otherwise the same absent blob.
    /// #15 needs to tell them apart: one means render it on demand, and the
    /// other means there is nothing to render.
    ///
    /// Defaulted rather than required, because a blob written before this
    /// field existed is an entry whose patches are where they should be.
    #[serde(default)]
    patches_omitted: bool,

    tree: DiffFileEntry,
}

/// The most one file's patch may weigh before an entry is written without it.
///
/// A file this big is one nobody reads in an answer anyway — the response
/// ceiling cuts it long before this — so what keeping it buys is a download
/// saved for a patch that will be truncated. #15 renders it on demand, which
/// is the same work the entry would have done and is only done if somebody
/// asks.
pub const PATCH_CAP: usize = 256 * 1024;

/// The most one entry may weigh before its patches are dropped from it.
///
/// A guard against pathological patch volume rather than against trees: a
/// package with four thousand files is a few hundred KB of tree, and the
/// tree is what is expensive to work out again. What this stops is one
/// comparison taking a thirtieth of the 256 MB budget and everything else's
/// place in it.
pub const ENTRY_CAP: usize = 8 * 1024 * 1024;

/// The interface the rest of the crate has to cached results.
pub struct DiffStore {
    source: Source,

    /// The most one patch may weigh. A field rather than the constant read
    /// where it is used, for the reason `Archive` carries its size limit: a
    /// cap can then be exercised with a small number and a real comparison
    /// instead of with a file nobody wants in a fixture set.
    patch_cap: usize,

    /// The most one entry may weigh, and a field for the same reason.
    entry_cap: usize,

    /// Where this store says it could not answer.
    ///
    /// Its own rather than the one a [`Ctx`](crate::tools::Ctx) carries,
    /// because a store outlives the call that reached it: a write is
    /// backgrounded, so its failure happens once the call's line is written
    /// and the context that carried it is gone. Both are the runtime logs in
    /// production, which is the only place either of them goes.
    log: Sink,
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
            Err(_) => Source::Unavailable(NO_CREDENTIALS),
        };

        Self::over(source)
    }

    /// A store over `source`, with the caps this project runs.
    fn over(source: Source) -> Self {
        Self {
            source,
            patch_cap: PATCH_CAP,
            entry_cap: ENTRY_CAP,
            log: Sink::default(),
        }
    }

    /// No store at all, naming why.
    ///
    /// What [`DiffStore::live`] falls back to: a deployment with no
    /// credentials to reach a store with is a server that computes every
    /// diff, which is the server that existed before there was a cache.
    pub fn unavailable() -> Self {
        Self::over(Source::Unavailable(NO_CREDENTIALS))
    }

    /// The same store, saying what it could not do to `log`.
    pub fn logging_to(self, log: Sink) -> Self {
        Self { log, ..self }
    }

    /// The same store, leaving out any patch over `bytes`.
    pub fn capping_patches_at(self, bytes: usize) -> Self {
        Self {
            patch_cap: bytes,
            ..self
        }
    }

    /// The same store, dropping the patches from any entry over `bytes`.
    pub fn capping_entries_at(self, bytes: usize) -> Self {
        Self {
            entry_cap: bytes,
            ..self
        }
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

        // Read rather than asked about, because the answer to "is it there"
        // and the answer to "what is in it" are the same request. An entry
        // whose patches are not there is a real entry and not half of one —
        // that is what an entry over the size cap is — so a missing
        // `patches.json` is no patches rather than no hit.
        let patches = match self.read(&key.patches_path()).await {
            Some(bytes) => serde_json::from_slice(&bytes).unwrap_or_default(),
            None => BTreeMap::new(),
        };

        Some(Entry {
            key: meta.key,
            tree: meta.tree,
            patches,
        })
    }

    /// Remember `entry`, after the answer has already gone.
    ///
    /// Not `async`, and that is the whole of it: a caller is waiting on a
    /// diff and not on a cache, so the work is handed to the runtime's
    /// `waitUntil` and this returns. What the caller spends on the cache is
    /// the lookup that missed.
    ///
    /// The registration is the platform's rather than a bare `tokio::spawn`,
    /// which is what it would otherwise be: the runtime drains what it was
    /// given at shutdown, so an entry whose write is still in flight when
    /// the function is stopped is still written.
    pub fn put(self: Arc<Self>, entry: Entry) {
        in_background(async move { self.writing(entry).await });
    }

    /// Write `entry`, both blobs or neither.
    ///
    /// The patches are serialised before anything is written, so an entry
    /// that cannot be written whole is not written at all rather than left
    /// as a `meta.json` whose patches never arrived.
    async fn writing(&self, entry: Entry) {
        // Measured on the patch's own text rather than on the JSON it
        // becomes. The two differ by a couple of dozen bytes of punctuation
        // and escaping against a quarter of a megabyte, and the text is the
        // number a reader of this can check against a file.
        let patches: BTreeMap<String, Patch> = entry
            .patches
            .into_iter()
            .filter(|(_, patch)| patch.data.len() <= self.patch_cap)
            .collect();

        let mut meta = Meta {
            key: entry.key,
            patches_omitted: false,
            tree: entry.tree,
        };

        let (Ok(bytes), Ok(patches)) = (serde_json::to_vec(&meta), serde_json::to_vec(&patches))
        else {
            return;
        };

        // Both blobs together, because the cap is on the entry. A tree that
        // is over it on its own keeps nothing by dropping its patches — but
        // the patches are the only half of an entry there is to drop, and
        // the tree is the half that is expensive to work out again.
        if bytes.len() + patches.len() > self.entry_cap {
            meta.patches_omitted = true;

            let Ok(bytes) = serde_json::to_vec(&meta) else {
                return;
            };

            self.write(&meta.key.meta_path(), bytes).await;
            return;
        }

        self.write(&meta.key.meta_path(), bytes).await;
        self.write(&meta.key.patches_path(), patches).await;
    }

    /// The bytes at `pathname`, if the store holds any.
    async fn read(&self, pathname: &str) -> Option<Vec<u8>> {
        match &self.source {
            Source::Live(api) => match api.read(pathname).await {
                Ok(found) => found,
                Err(_) => self.gave_up(READING),
            },
            Source::Memory(memory) => memory.read(pathname),
            Source::Unavailable(why) => self.gave_up(why),
        }
    }

    /// Put `bytes` at `pathname`, or do not.
    async fn write(&self, pathname: &str, bytes: Vec<u8>) {
        match &self.source {
            Source::Live(api) => {
                if api.put(pathname, bytes).await.is_err() {
                    self.gave_up(WRITING);
                }
            }
            Source::Memory(memory) => memory.write(pathname, bytes).await,
            Source::Unavailable(why) => {
                self.gave_up(why);
            }
        }
    }

    /// Say that the cache could not do `doing`, and answer nothing.
    ///
    /// Returns the miss rather than only writing the note, so that the two
    /// cannot come apart: a path that gave up without saying so is a cache
    /// that has quietly stopped working, which looks exactly like a cache
    /// that is working and cold.
    fn gave_up(&self, doing: &'static str) -> Option<Vec<u8>> {
        self.log.note(&Note {
            seam: "store",
            doing,
        });
        None
    }
}

/// Run `work` after the answer has gone, and let the runtime drain it.
///
/// `AppState` is the runtime's own handle on the process-global collector
/// `run` waits on at shutdown, so this is `waitUntil` and not an imitation
/// of it. Built here rather than taken from the request because `VercelLayer`
/// drops the state on its way into `axum`: nothing on this side of that layer
/// is ever handed one, and the collector is the process's regardless.
fn in_background(work: impl Future<Output = ()> + Send + 'static) {
    // Three arguments on every platform this deploys or builds on. The
    // runtime gives the type a different constructor off unix, which this
    // repository has no target for.
    AppState::new(LogContext::new(None, None, None)).wait_until(work);
}

/// What a store with no credentials to reach one with says it was doing.
const NO_CREDENTIALS: &str = "reading the blob store's credentials";

/// What a failed read says it was doing.
const READING: &str = "reading a cached result from the blob store";

/// What a failed write says it was doing.
const WRITING: &str = "writing to the blob store";

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
pub struct Memory {
    blobs: Arc<Mutex<BTreeMap<String, Vec<u8>>>>,

    /// How long this store takes over one blob.
    ///
    /// Nothing, unless a test asks for otherwise. It is what makes "the
    /// answer did not wait for the write" a measurement: an in-process map
    /// is written faster than a response can be built, so without it the two
    /// orderings look the same.
    stall: Duration,
}

impl Memory {
    pub fn new() -> Self {
        Self::default()
    }

    /// The same store, taking `delay` over every blob it writes.
    pub fn stalling(self, delay: Duration) -> Self {
        Self {
            stall: delay,
            ..self
        }
    }

    /// The blob at `pathname`, if this store holds one.
    ///
    /// What a `patches.json` holds is the contract #15 reads a rendered patch
    /// out of and #27 reads a result out of, and nothing else in this process
    /// can see it — so a suite that could only count the blobs would be
    /// pinning where they are and not what they say.
    pub fn blob(&self, pathname: &str) -> Option<Vec<u8>> {
        self.read(pathname)
    }

    /// Every pathname this store holds, in the order the store keeps them.
    pub fn written(&self) -> Vec<String> {
        self.blobs
            .lock()
            .map(|blobs| blobs.keys().cloned().collect())
            .unwrap_or_default()
    }

    /// The store that writes here, to hand to a [`Ctx`](crate::tools::Ctx).
    pub fn store(&self) -> DiffStore {
        DiffStore::over(Source::Memory(self.clone()))
    }

    fn read(&self, pathname: &str) -> Option<Vec<u8>> {
        self.blobs.lock().ok()?.get(pathname).cloned()
    }

    async fn write(&self, pathname: &str, bytes: Vec<u8>) {
        if !self.stall.is_zero() {
            tokio::time::sleep(self.stall).await;
        }

        if let Ok(mut blobs) = self.blobs.lock() {
            blobs.insert(pathname.to_owned(), bytes);
        }
    }
}
