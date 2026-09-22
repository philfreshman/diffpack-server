//! Cached diff results.
//!
//! [`DiffStore`] is what the rest of the crate sees of the cache — get the
//! entry for a `DiffKey`, put an entry — and everything underneath it is
//! this module's: the Vercel Blob client, the 256 MB budget and the eviction
//! that keeps it. See [ADR
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
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
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
    /// Whatever comes to read an entry back has to tell them apart: one
    /// means render it on demand, the other means there is nothing to
    /// render. `get_file_diff` renders on demand every time today and will
    /// want this the moment it looks in the store first.
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

/// The most this project's blob store may hold. Never exceeded.
pub const CACHE_MAX_BYTES: u64 = 256 * 1024 * 1024;

/// What a sweep evicts down to, leaving the gap up to [`CACHE_MAX_BYTES`].
///
/// The 16 MiB is not decoration. Two invocations can admit at the same
/// moment, each having read a total that did not include the other's entry;
/// and Vercel Blob takes up to a minute to propagate a delete, so a listing
/// taken after a sweep can still count blobs that are already gone. Size
/// accounting here is a good estimate and never a fact, and a design that
/// treats it as one exceeds the ceiling exactly once, in production,
/// unobserved.
pub const CACHE_TARGET_BYTES: u64 = 240 * 1024 * 1024;

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

    /// The most this store may hold, and a field for the same reason again:
    /// a budget of a few kilobytes and a real comparison exercise the sweep
    /// that a budget of 256 MB and eight thousand of them would.
    max_bytes: u64,

    /// What a sweep leaves this store at. See [`CACHE_TARGET_BYTES`].
    target_bytes: u64,

    /// The prefix this store's entries live under, and the one a sweep
    /// counts against the budget.
    ///
    /// This build's schema in anything that ships — a bump moves the
    /// entries, and a sweep still reading the old prefix would be counting a
    /// store it no longer writes to. It is a field so that the one test that
    /// runs a sweep against the real blob store can scope itself to a prefix
    /// of its own, rather than evicting the cache this project serves from.
    ///
    /// Asked of [`crate::cache_key::prefix`] rather than spelled here. The
    /// blob layout is that module's — it is where a `DiffKey` becomes the
    /// two pathnames this one sweeps — and a `diffs/v{n}/` written out in
    /// this file would be a second copy of it to keep in step.
    prefix: String,

    /// Where this store says it could not answer.
    ///
    /// Its own rather than the one a [`Ctx`](crate::tools::Ctx) carries,
    /// because a store outlives the call that reached it: a write is
    /// backgrounded, so its failure happens once the call's line is written
    /// and the context that carried it is gone. Both are the runtime logs in
    /// production, which is the only place either of them goes.
    log: Sink,
}

/// What a store says about a blob at a pathname.
///
/// Three answers rather than a `bool`, because a store that could not say is
/// not a store that said no, and a write turns on the difference: a blob
/// that is there is one to skip, and a question that went unanswered is a
/// write to abandon. The entry may well be there, and the cost of skipping a
/// write that was needed is one more recomputed diff — where the cost of
/// making one that was not is an `uploaded_at` reset on an entry eviction
/// orders by.
///
/// Named for the question rather than for [`Held`], which is one blob a
/// [`Memory`] is keeping. This is an answer about a blob and not a blob.
enum Presence {
    /// The blob is there, so there is nothing to write and no room to ask
    /// for.
    There,

    /// The blob is not there.
    Missing,

    /// The store did not say, and has already left a note saying so.
    Unknown,
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
            max_bytes: CACHE_MAX_BYTES,
            target_bytes: CACHE_TARGET_BYTES,
            prefix: crate::cache_key::prefix(crate::cache_key::SCHEMA),
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

    /// The same store, holding at most `max` bytes and sweeping down to
    /// `target` when admitting an entry would take it past `max`.
    ///
    /// Two numbers rather than one, because the gap between them is what
    /// absorbs the two things that make a hard-edged check unreliable — see
    /// [`CACHE_TARGET_BYTES`].
    pub fn budgeting(self, max: u64, target: u64) -> Self {
        Self {
            max_bytes: max,
            target_bytes: target,
            ..self
        }
    }

    /// The same store, counting and sweeping the entries under `prefix`.
    ///
    /// Only the networked test below, which needs a corner of the real store
    /// that is not the one this project caches into.
    #[cfg(test)]
    fn sweeping(self, prefix: impl Into<String>) -> Self {
        Self {
            prefix: prefix.into(),
            ..self
        }
    }

    /// Whether this deployment has a store at all.
    ///
    /// Not whether a lookup will find anything, and not whether one will
    /// succeed: a store that is there can still fail a read, and that failure
    /// is a miss like any other by the rule in the module header. This is the
    /// narrower fact settled when the store was built — whether there were
    /// credentials to build a client from — and it is the same answer for
    /// every call this instance serves.
    ///
    /// It exists for the log and nothing else. A caller acts identically
    /// either way, which is the whole of why [`DiffStore::get`] has no
    /// `Result`; what an operator needs is to tell a cache that is cold from
    /// a deployment that has none, and those two are the same flat hundred
    /// percent miss without it.
    pub fn is_available(&self) -> bool {
        !matches!(self.source, Source::Unavailable(_))
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
        // and the answer to "what is in it" are the same request.
        //
        // A `patches.json` that is not there is one of two things, and the
        // flag above is what tells them apart. An entry over the size cap
        // says its patches were dropped and is whole without them. An entry
        // that says nothing is half of one — a `meta.json` whose partner
        // never arrived, which is what a write that failed between the two
        // leaves behind — and answering with it would serve a comparison
        // whose every patch is silently missing. So it is a miss, and the
        // miss is what repairs it: recomputing heads past the `meta.json`
        // that is there and writes the blob that is not.
        let patches = match self.read(&key.patches_path()).await {
            Some(bytes) => serde_json::from_slice(&bytes).ok()?,
            None if meta.patches_omitted => BTreeMap::new(),
            None => return None,
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

            self.writing_whatever_is_missing(vec![(meta.key.meta_path(), bytes)])
                .await;
            return;
        }

        // Both blobs together: they are one entry, and an entry admitted by
        // halves is the thing the budget is measured in arriving in a shape
        // the budget cannot see.
        self.writing_whatever_is_missing(vec![
            (meta.key.meta_path(), bytes),
            (meta.key.patches_path(), patches),
        ])
        .await;
    }

    /// Write every one of `blobs` this store does not already hold, inside
    /// the budget.
    ///
    /// # Why the store is asked what it holds before it is asked for room
    ///
    /// A write skips a blob that is already at its pathname — see
    /// [`DiffStore::write`] — so room asked for before that is known is room
    /// a write can decline to use. An entry put a second time would then
    /// evict other comparisons to make space it never puts anything in: the
    /// cache ends up smaller and holding what it would have held anyway.
    ///
    /// Sharper than wasteful, because the entry being written is in the
    /// listing the sweep reads and may be the oldest thing in it. The room
    /// it asks for can be freed by deleting the very blobs the write is
    /// about to skip — and a delete this store has not finished propagating
    /// is a head that still sees a blob on its way out, so the write skips,
    /// the delete lands, and the entry is gone. Bounded rather than silent:
    /// [`DiffStore::get`] reads half an entry as a miss and the miss rewrites
    /// it, so the cost is a recomputed diff. It is still a sweep spent for
    /// nothing.
    ///
    /// So the heads come first, and an entry already there is not admitted
    /// at all — there is no room to ask for, because nothing is going to be
    /// written. What that costs is a head per blob on a write that does go
    /// ahead, on work the runtime drains after the answer has gone and that
    /// no caller is waiting on. What it saves on a write that does not is
    /// the listing and every delete a sweep would have made.
    ///
    /// The second head, inside [`DiffStore::write`], is not the first one
    /// repeated. A sweep happens between them, and it is long enough for
    /// another invocation to land the blob this one is about to write; the
    /// head immediately before the put is what keeps that from resetting the
    /// moment a blob was uploaded, which is the order eviction runs in.
    ///
    /// Blobs whose pathname this store cannot answer for abandon the whole
    /// write rather than their own half of it. A head that failed is not a
    /// blob that is missing, and writing the rest of the entry around it is
    /// how a `patches.json` ends up without the `meta.json` that names it —
    /// an orphan nothing ever reads and the budget counts forever.
    async fn writing_whatever_is_missing(&self, blobs: Vec<(String, Vec<u8>)>) {
        let mut missing = Vec::with_capacity(blobs.len());
        for (pathname, bytes) in blobs {
            match self.holds(&pathname).await {
                Presence::There => {}
                Presence::Missing => missing.push((pathname, bytes)),
                Presence::Unknown => return,
            }
        }

        let incoming: u64 = missing.iter().map(|(_, bytes)| bytes.len() as u64).sum();
        if incoming == 0 {
            return;
        }

        if !self.admitting(incoming).await {
            return;
        }

        for (pathname, bytes) in missing {
            self.write(&pathname, bytes).await;
        }
    }

    /// Make room for an entry of `incoming` bytes, and say whether there is
    /// any.
    ///
    /// The budget is a ceiling over a store this process shares with every
    /// other invocation, so the total is read from the store rather than
    /// carried: there is nowhere to carry it that two functions would agree
    /// on. What that costs is one listing per write, and what it buys is a
    /// number that is true of the store rather than of this process.
    async fn admitting(&self, incoming: u64) -> bool {
        // Refused before anything is listed, let alone deleted. A sweep for
        // an entry that would not fit in an empty store spends every other
        // comparison in the cache and still has no room at the end of it.
        if incoming > self.max_bytes {
            self.gave_up::<()>(TOO_BIG);
            return false;
        }

        // A listing that could not be taken is a total that is not known,
        // and admitting against a total that is not known is how a ceiling
        // gets exceeded. The cost of refusing is one entry not cached,
        // which is the cost of every other failure in this module.
        let Some(blobs) = self.listed().await else {
            return false;
        };

        let mut total: u64 = blobs.iter().map(|blob| blob.size).sum();
        if total + incoming <= self.max_bytes {
            return true;
        }

        // Down to the target rather than to the ceiling, so that the next
        // write is not another sweep and two of them at once have room to
        // overlap in. See [`CACHE_TARGET_BYTES`].
        for entry in oldest_first(&blobs) {
            if total + incoming <= self.target_bytes {
                break;
            }

            if self.remove(&entry.blobs).await {
                total = total.saturating_sub(entry.bytes);
            }
        }

        // The sweep can run out of entries before it runs out of work —
        // every delete having failed is the plain case — and a sweep that
        // freed nothing is a total that still has no room in it. Admitting
        // here because a sweep was attempted is how the one number this
        // issue is written around gets exceeded.
        total + incoming <= self.max_bytes
    }

    /// Every blob this store holds, or nothing if it could not say.
    async fn listed(&self) -> Option<Vec<blob::Blob>> {
        match &self.source {
            Source::Live(api) => match api.list(&self.prefix).await {
                Ok(blobs) => Some(blobs),
                Err(_) => self.gave_up(LISTING),
            },
            Source::Memory(memory) => Some(memory.list(&self.prefix).await),
            Source::Unavailable(why) => self.gave_up(why),
        }
    }

    /// Delete every blob at `pathnames`, and say whether they are gone.
    ///
    /// All of them in one call, because they are one entry: a delete that
    /// took them one at a time could leave half an entry behind when the
    /// second failed.
    ///
    /// The answer is what a sweep counts in. Bytes credited to a delete that
    /// failed are room the store does not have, which is the same mistake as
    /// admitting against a listing that was never taken.
    async fn remove(&self, pathnames: &[&str]) -> bool {
        match &self.source {
            Source::Live(api) => match api.delete(pathnames).await {
                Ok(()) => true,
                Err(_) => {
                    self.gave_up::<()>(DELETING);
                    false
                }
            },
            Source::Memory(memory) => {
                let gone = memory.delete(pathnames).await;
                if !gone {
                    self.gave_up::<()>(DELETING);
                }
                gone
            }
            Source::Unavailable(why) => {
                self.gave_up::<()>(why);
                false
            }
        }
    }

    /// Whether this store holds a blob at `pathname`.
    ///
    /// Not [`DiffStore::read`], and the difference is what a head is for: the
    /// question is whether a blob is there, and downloading one to find out
    /// would be paying for an entry's bytes to decide not to write them.
    async fn holds(&self, pathname: &str) -> Presence {
        match &self.source {
            Source::Live(api) => match api.head(pathname).await {
                Ok(Some(_)) => Presence::There,
                Ok(None) => Presence::Missing,
                Err(_) => {
                    self.gave_up::<()>(WRITING);
                    Presence::Unknown
                }
            },
            Source::Memory(memory) => match memory.holds(pathname) {
                true => Presence::There,
                false => Presence::Missing,
            },
            Source::Unavailable(why) => {
                self.gave_up::<()>(why);
                Presence::Unknown
            }
        }
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

    /// Put `bytes` at `pathname`, unless something is already there.
    ///
    /// The head is not an optimisation. An entry is derived from its
    /// contents, so a blob already at this pathname holds these bytes
    /// already — and writing them again would reset the moment it was
    /// uploaded, which is the order eviction runs in. A comparison asked for
    /// often would keep moving to the back of that queue.
    ///
    /// What makes that happen at all is a read that missed although it
    /// should not have: a lookup that failed, or two invocations working out
    /// the same comparison at once.
    async fn write(&self, pathname: &str, bytes: Vec<u8>) {
        match &self.source {
            Source::Live(api) => {
                match api.head(pathname).await {
                    Ok(Some(_)) => return,
                    Ok(None) => {}
                    // A head that could not be answered is not a reason to
                    // overwrite: the entry may well be there, and the cost
                    // of skipping a write that was needed is one more
                    // recomputed diff.
                    Err(_) => {
                        self.gave_up::<()>(WRITING);
                        return;
                    }
                }

                if api.put(pathname, bytes).await.is_err() {
                    self.gave_up::<()>(WRITING);
                }
            }
            Source::Memory(memory) => {
                if !memory.holds(pathname) {
                    memory.write(pathname, bytes).await;
                }
            }
            Source::Unavailable(why) => {
                self.gave_up::<()>(why);
            }
        }
    }

    /// Say that the cache could not do `doing`, and answer nothing.
    ///
    /// Returns the miss rather than only writing the note, so that the two
    /// cannot come apart: a path that gave up without saying so is a cache
    /// that has quietly stopped working, which looks exactly like a cache
    /// that is working and cold.
    fn gave_up<T>(&self, doing: &'static str) -> Option<T> {
        self.log.note(&Note {
            seam: "store",
            doing,
        });
        None
    }
}

/// One cached entry as a listing shows it.
///
/// Built from blobs rather than read: the store holds blobs, and an entry is
/// this module's grouping of them. There is no index to consult and none
/// wanted — a second record of what the store holds is a second record to
/// keep in step with it.
struct Listed<'a> {
    /// What the whole entry weighs.
    bytes: u64,

    /// When the entry began to exist, which is its oldest blob's moment.
    /// The two are written one after the other, so the later one is when
    /// the entry was finished rather than when it arrived.
    uploaded_at: &'a str,

    /// Every blob the entry is, so that they go together.
    blobs: Vec<&'a str>,
}

/// `blobs` grouped into entries, oldest first.
///
/// The `diff_id` is the third segment of a pathname — `diffs/v1/{id}/…` —
/// and a blob whose pathname has no third segment is not one of this
/// module's, so it is counted towards the total and never deleted. The
/// budget is over the store, and something else's blob still takes up room
/// in it.
fn oldest_first(blobs: &[blob::Blob]) -> Vec<Listed<'_>> {
    let mut entries: BTreeMap<&str, Listed<'_>> = BTreeMap::new();

    for blob in blobs {
        let Some(diff_id) = blob.pathname.split('/').nth(2) else {
            continue;
        };

        let entry = entries.entry(diff_id).or_insert_with(|| Listed {
            bytes: 0,
            uploaded_at: &blob.uploaded_at,
            blobs: Vec::new(),
        });

        entry.bytes += blob.size;
        entry.uploaded_at = entry.uploaded_at.min(&blob.uploaded_at);
        entry.blobs.push(&blob.pathname);
    }

    let mut entries: Vec<Listed<'_>> = entries.into_values().collect();

    // The store's own ISO-8601 to the millisecond, whose lexical order is
    // its chronological one. Two entries in the same millisecond are
    // separated by the blobs they are, so that a sweep of the same listing
    // twice deletes the same entries.
    entries.sort_by(|left, right| {
        (left.uploaded_at, &left.blobs).cmp(&(right.uploaded_at, &right.blobs))
    });

    entries
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

/// What a failed listing says it was doing.
const LISTING: &str = "listing what the blob store holds";

/// What a failed eviction says it was doing.
const DELETING: &str = "evicting an entry from the blob store";

/// What a store refusing an entry it could never hold says it was doing.
const TOO_BIG: &str = "admitting an entry larger than the whole budget";

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
    blobs: Arc<Mutex<BTreeMap<String, Held>>>,

    /// How long this store takes over one blob it lists, deletes or writes.
    ///
    /// Nothing, unless a test asks for otherwise. It is what makes "the
    /// answer did not wait for the write" a measurement: an in-process map
    /// is written faster than a response can be built, so without it the two
    /// orderings look the same. A sweep is the same measurement over the
    /// more expensive half — a listing of the whole store and a delete per
    /// entry it takes.
    stall: Duration,

    /// Whether every read of this store answers as a miss.
    ///
    /// What a lookup that failed looks like, and what two invocations
    /// computing one comparison at once look like to each other. Both end in
    /// a write over an entry that is already there, which is the one thing
    /// the head before a put is there to stop.
    lose_reads: bool,

    /// How many more deletes this store takes, if it is refusing them.
    ///
    /// `None` unless a test asks for otherwise, and then it is the number of
    /// deletes that still work before every one after them fails. What a
    /// real store does now and then, and the one state a test cannot reach
    /// through the wire: an in-process map cannot fail to forget a blob, so
    /// without this nothing can tell a sweep that credited itself bytes it
    /// never freed from one that did not.
    ///
    /// Shared between the stores a view hands out, because a sweep's deletes
    /// are one run against one allowance.
    lose_deletes: Option<Arc<AtomicUsize>>,
}

/// One blob this store holds.
///
/// `uploaded_at` is the moment the store took it, which is the blob field
/// eviction orders on. It counts rather than reads a clock: two writes in
/// one millisecond are indistinguishable by a timestamp and not by this,
/// and zero-padding keeps the lexical order the real store's ISO-8601
/// already has.
#[derive(Debug, Clone)]
struct Held {
    bytes: Vec<u8>,
    uploaded_at: String,
}

/// How many blobs this process has taken, across every [`Memory`].
///
/// One counter for all of them, because what it stands in for is a clock and
/// a clock is not per store either.
static UPLOADS: AtomicU64 = AtomicU64::new(0);

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

    /// `store`'s blobs, answering every read of them as a miss.
    ///
    /// Written as a view of another store rather than as a flag on one,
    /// because what it stands for is a second reader of the same blobs — an
    /// invocation that cannot see what the first has written.
    pub fn losing_reads(store: &Self) -> Self {
        Self {
            lose_reads: true,
            ..store.clone()
        }
    }

    /// `store`'s blobs, refusing every delete of them.
    ///
    /// What the budget does with a delete it did not get is the half of a
    /// sweep nothing else can state: bytes credited to a delete that never
    /// happened are room the store does not have, and an entry admitted
    /// against them is the ceiling exceeded by the code that keeps it.
    pub fn losing_deletes(store: &Self) -> Self {
        Self::losing_deletes_after(store, 0)
    }

    /// `store`'s blobs, taking `kept` deletes and refusing every one after.
    ///
    /// A sweep that freed some of what it needed and not all of it, which is
    /// the likelier failure than none of it: the store is smaller afterwards
    /// and still has no room.
    pub fn losing_deletes_after(store: &Self, kept: usize) -> Self {
        Self {
            lose_deletes: Some(Arc::new(AtomicUsize::new(kept))),
            ..store.clone()
        }
    }

    /// When the blob at `pathname` was taken, if this store holds one.
    pub fn uploaded_at(&self, pathname: &str) -> Option<String> {
        Some(self.blobs.lock().ok()?.get(pathname)?.uploaded_at.clone())
    }

    /// The blob at `pathname`, if this store holds one.
    ///
    /// What a `patches.json` holds is the contract a rendered patch will be
    /// read out of — `get_file_diff` once it looks in the store, #27 from
    /// TypeScript — and nothing else in this process can see it, so a suite
    /// that could only count the blobs would be pinning where they are and
    /// not what they say.
    pub fn blob(&self, pathname: &str) -> Option<Vec<u8>> {
        self.read(pathname)
    }

    /// Lose the blob at `pathname`.
    ///
    /// What a write that failed after its partner landed leaves behind, and
    /// the one state nothing else here can put a store in: the two blobs of
    /// an entry are written one after the other, so a `meta.json` whose
    /// `patches.json` never arrived is a real outcome and not an invented
    /// one.
    pub fn forget(&self, pathname: &str) {
        if let Ok(mut blobs) = self.blobs.lock() {
            blobs.remove(pathname);
        }
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
        if self.lose_reads {
            return None;
        }

        Some(self.blobs.lock().ok()?.get(pathname)?.bytes.clone())
    }

    /// Every blob this store holds under `prefix`.
    ///
    /// The three fields a real listing carries and no others, because they
    /// are the three the budget is built on. A store whose reads are lost
    /// still answers this, for the reason [`Memory::holds`] does: losing a
    /// download is not forgetting what is there.
    async fn list(&self, prefix: &str) -> Vec<blob::Blob> {
        self.stalled().await;

        let Ok(blobs) = self.blobs.lock() else {
            return Vec::new();
        };

        blobs
            .iter()
            .filter(|(pathname, _)| pathname.starts_with(prefix))
            .map(|(pathname, held)| blob::Blob {
                pathname: pathname.clone(),
                size: held.bytes.len() as u64,
                uploaded_at: held.uploaded_at.clone(),
            })
            .collect()
    }

    /// Lose every blob at `pathnames`, and say whether they are gone.
    ///
    /// The answer a real store gives, because a sweep counts in it. A store
    /// refusing deletes hands out its allowance until there is none left and
    /// says no from then on, leaving the blobs where they are.
    async fn delete(&self, pathnames: &[&str]) -> bool {
        self.stalled().await;

        if let Some(allowance) = &self.lose_deletes {
            let taken = allowance.fetch_update(Ordering::Relaxed, Ordering::Relaxed, |left| {
                left.checked_sub(1)
            });

            if taken.is_err() {
                return false;
            }
        }

        if let Ok(mut blobs) = self.blobs.lock() {
            for pathname in pathnames {
                blobs.remove(*pathname);
            }
        }

        true
    }

    /// Take as long over this as the store was asked to.
    ///
    /// Every operation a write performs and none that a read does. What the
    /// delay stands for is a request to somebody else's service, and a
    /// lookup makes one of those too — but the lookup is the half of the
    /// cache a caller *is* waiting on, so a store that stalled it would be
    /// measuring the opposite of what it was built to measure.
    async fn stalled(&self) {
        if !self.stall.is_zero() {
            tokio::time::sleep(self.stall).await;
        }
    }

    /// Whether this store holds a blob at `pathname`.
    ///
    /// Not [`Memory::read`], and the difference is the whole of what a head
    /// before a put is for: a store whose reads are lost still knows what it
    /// holds, exactly as a blob store whose download failed still answers
    /// this.
    fn holds(&self, pathname: &str) -> bool {
        self.blobs
            .lock()
            .is_ok_and(|blobs| blobs.contains_key(pathname))
    }

    async fn write(&self, pathname: &str, bytes: Vec<u8>) {
        self.stalled().await;

        if let Ok(mut blobs) = self.blobs.lock() {
            let uploaded_at = format!("{:020}", UPLOADS.fetch_add(1, Ordering::Relaxed));
            blobs.insert(pathname.to_owned(), Held { bytes, uploaded_at });
        }
    }
}

// ---------------------------------------------------------------------------
// Against the real store
// ---------------------------------------------------------------------------
//
// The only test in this file, and it is here rather than in `tests/` for the
// reason `src/store/blob.rs`'s are: a sweep runs against the Blob client, and
// ADR 0003 keeps that client private to this module, so there is nowhere
// outside it to write this from.
//
// Everything the sweep decides is proven at the wire in `tests/store.rs`,
// against a store that keeps its blobs in this process. What that cannot
// state is the half the store owns: that a real `uploadedAt` sorts the way
// this code assumes, that a real `size` is the number the budget is counted
// in, and that a blob a delete took is gone from a later listing. Those are
// facts about somebody else's service, and only it can settle them.
#[cfg(test)]
mod tests {
    use std::time::{Duration, SystemTime, UNIX_EPOCH};

    use super::*;

    /// A sweep against the real store leaves what it said it would leave.
    ///
    /// Seeded past the cap and summed from a real listing afterwards, which
    /// is the criterion's own wording: a total the sweep worked out cannot
    /// disagree with the sweep.
    ///
    /// Scoped to a prefix of its own. Eviction is oldest-first, so a sweep
    /// run at the prefix this project caches under would take the oldest
    /// real entries first and this test would be a cache flush with an
    /// assertion on the end of it.
    ///
    /// The listing is polled rather than read once. A delete takes up to a
    /// minute to propagate, which is one of the two reasons the budget keeps
    /// a gap under its ceiling at all — so a test that read the listing once
    /// would be asserting against the very staleness the design is built to
    /// tolerate.
    #[tokio::test]
    #[ignore = "networked: writes to this project's Vercel Blob store"]
    async fn a_sweep_against_the_real_store_leaves_the_newest_entry() {
        let credentials = blob::Credentials::from_env()
            .expect("VERCEL_OIDC_TOKEN with BLOB_STORE_ID, or BLOB_READ_WRITE_TOKEN");
        let seeding = blob::Api::live(credentials);

        // Unique per run: two runs at once must not sweep each other, and a
        // write refuses to overwrite.
        let run = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("the clock is set after 1970")
            .as_nanos();

        // Three segments before the filename, as `diffs/v1/{diff_id}/` has,
        // because the third is what an entry is grouped by.
        let prefix = format!("tests/sweep-{run}/");

        // Four entries, oldest first, each the two blobs an entry is.
        let half = vec![b'x'; 4096];
        let entry = 2 * half.len() as u64;
        let named: Vec<String> = (0..4).map(|n| format!("{n:064}")).collect();

        for id in &named {
            for file in ["meta.json", "patches.json"] {
                seeding
                    .put(&format!("{prefix}{id}/{file}"), half.clone())
                    .await
                    .expect("the store takes a write at a pathname of ours");
            }
        }

        // Room for three, swept down to two, and a fifth arriving: the three
        // oldest have to go.
        let credentials = blob::Credentials::from_env().expect("the same two variables");
        let store = DiffStore::over(Source::Live(blob::Api::live(credentials)))
            .budgeting(3 * entry, 2 * entry)
            .sweeping(&prefix);

        assert!(
            store.admitting(entry).await,
            "an entry this size fits inside the budget once the sweep has run"
        );

        let reading =
            blob::Api::live(blob::Credentials::from_env().expect("the same two variables"));
        let survived = settled(&reading, &prefix, entry).await;

        assert_eq!(
            survived,
            vec![
                format!("{prefix}{}/meta.json", named[3]),
                format!("{prefix}{}/patches.json", named[3])
            ],
            "the newest entry, both of its blobs, and nothing else"
        );

        for id in &named {
            let paths = [
                format!("{prefix}{id}/meta.json"),
                format!("{prefix}{id}/patches.json"),
            ];
            let paths: Vec<&str> = paths.iter().map(String::as_str).collect();
            let _ = reading.delete(&paths).await;
        }
    }

    /// The pathnames under `prefix`, once the listing weighs `expected`.
    ///
    /// A delete is eventually consistent here, so the listing catches up
    /// rather than being right immediately. Two minutes is twice the window
    /// the service documents.
    async fn settled(api: &blob::Api, prefix: &str, expected: u64) -> Vec<String> {
        let mut listed = Vec::new();

        for _ in 0..120 {
            let blobs = api
                .list(prefix)
                .await
                .expect("the store lists a prefix of ours");

            let held: u64 = blobs.iter().map(|blob| blob.size).sum();
            listed = blobs.iter().map(|blob| blob.pathname.clone()).collect();
            listed.sort();

            if held == expected {
                return listed;
            }

            tokio::time::sleep(Duration::from_secs(1)).await;
        }

        panic!("the listing should weigh {expected} bytes by now, it holds {listed:?}");
    }
}
