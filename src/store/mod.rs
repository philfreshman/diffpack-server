mod blob;

use std::collections::BTreeMap;
use std::future::Future;
use std::ops::Range;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use vercel_runtime::{AppState, LogContext};

use serde::{Deserialize, Serialize};

use crate::cache_key::DiffKey;
use crate::engine::{DiffFileEntry, Patch};
use crate::error::Failure;
use crate::log::{Note, Sink};

#[derive(Debug, Clone)]
pub struct Entry {
    pub key: DiffKey,

    pub tree: DiffFileEntry,

    pub patches: BTreeMap<String, Patch>,
}

#[derive(Debug, Serialize, Deserialize)]
struct Meta {
    #[serde(flatten)]
    key: DiffKey,

    #[serde(default)]
    patches_omitted: bool,

    tree: DiffFileEntry,
}

pub const PATCH_CAP: usize = 256 * 1024;

pub const ENTRY_CAP: usize = 8 * 1024 * 1024;

pub const CACHE_MAX_BYTES: u64 = 256 * 1024 * 1024;

pub const CACHE_TARGET_BYTES: u64 = 240 * 1024 * 1024;

const THE_TREE_THEN_ITS_PATCHES: u32 = 2;

pub const LONGEST_LOOKUP: Duration = blob::LONGEST_READ.saturating_mul(THE_TREE_THEN_ITS_PATCHES);

pub struct DiffStore {
    source: Source,

    patch_cap: usize,

    entry_cap: usize,

    max_bytes: u64,

    target_bytes: u64,

    prefix: String,

    log: Sink,
}

enum Presence {
    There,

    Missing,

    Unknown,
}

enum Source {
    Blobs(Blobs),

    Unavailable(&'static str),
}

enum Blobs {
    Live(blob::Api),

    Memory(Memory),
}

impl Blobs {
    async fn list(&self, prefix: &str) -> Result<Vec<blob::Blob>, Failure> {
        match self {
            Self::Live(api) => api.list(prefix).await,
            Self::Memory(memory) => memory.list(prefix).await,
        }
    }

    async fn head(&self, pathname: &str) -> Result<bool, Failure> {
        match self {
            Self::Live(api) => Ok(api.head(pathname).await?.is_some()),
            Self::Memory(memory) => memory.head(pathname).await,
        }
    }

    async fn read(&self, pathname: &str) -> Result<Option<Vec<u8>>, Failure> {
        match self {
            Self::Live(api) => api.read(pathname).await,
            Self::Memory(memory) => memory.read(pathname).await,
        }
    }

    async fn write(&self, pathname: &str, bytes: Vec<u8>) -> Result<(), Failure> {
        match self {
            Self::Live(api) => api.put(pathname, bytes).await,
            Self::Memory(memory) => memory.write(pathname, bytes).await,
        }
    }

    async fn delete(&self, pathnames: &[&str]) -> Result<(), Failure> {
        match self {
            Self::Live(api) => api.delete(pathnames).await,
            Self::Memory(memory) => memory.delete(pathnames).await,
        }
    }
}

impl DiffStore {
    pub fn live() -> Self {
        let source = match blob::Credentials::from_env() {
            Ok(credentials) => Source::Blobs(Blobs::Live(blob::Api::live(credentials))),
            Err(_) => Source::Unavailable(NO_CREDENTIALS),
        };

        Self::over(source)
    }

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

    pub fn unavailable() -> Self {
        Self::over(Source::Unavailable(NO_CREDENTIALS))
    }

    pub fn logging_to(self, log: Sink) -> Self {
        Self { log, ..self }
    }

    pub fn capping_patches_at(self, bytes: usize) -> Self {
        Self {
            patch_cap: bytes,
            ..self
        }
    }

    pub fn capping_entries_at(self, bytes: usize) -> Self {
        Self {
            entry_cap: bytes,
            ..self
        }
    }

    pub fn budgeting(self, max: u64, target: u64) -> Self {
        Self {
            max_bytes: max,
            target_bytes: target,
            ..self
        }
    }

    #[cfg(test)]
    fn sweeping(self, prefix: impl Into<String>) -> Self {
        Self {
            prefix: prefix.into(),
            ..self
        }
    }

    pub fn is_available(&self) -> bool {
        !matches!(self.source, Source::Unavailable(_))
    }

    pub async fn get(&self, key: &DiffKey) -> Option<Entry> {
        let meta = self.read(&key.meta_path()).await?;
        let meta: Meta = serde_json::from_slice(&meta).ok()?;

        if &meta.key != key {
            return None;
        }

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

    pub fn put(self: Arc<Self>, entry: Entry) {
        in_background(async move { self.writing(entry).await });
    }

    async fn writing(&self, entry: Entry) {
        let rendered = entry.patches.len();

        let patches: BTreeMap<String, Patch> = entry
            .patches
            .into_iter()
            .filter(|(_, patch)| patch.data.len() <= self.patch_cap)
            .collect();

        let mut meta = Meta {
            key: entry.key,
            patches_omitted: patches.len() < rendered,
            tree: entry.tree,
        };

        let (Ok(bytes), Ok(patches)) = (serde_json::to_vec(&meta), serde_json::to_vec(&patches))
        else {
            return;
        };

        if bytes.len() + patches.len() > self.entry_cap {
            meta.patches_omitted = true;

            let Ok(bytes) = serde_json::to_vec(&meta) else {
                return;
            };

            self.writing_whatever_is_missing(vec![(meta.key.meta_path(), bytes)])
                .await;
            return;
        }

        self.writing_whatever_is_missing(vec![
            (meta.key.meta_path(), bytes),
            (meta.key.patches_path(), patches),
        ])
        .await;
    }

    async fn writing_whatever_is_missing(&self, entry: Vec<(String, Vec<u8>)>) {
        let Some(blobs) = self.blobs() else {
            return;
        };

        let mut missing = Vec::with_capacity(entry.len());
        for (pathname, bytes) in entry {
            match self.holds(blobs, &pathname).await {
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
            self.write(blobs, &pathname, bytes).await;
        }
    }

    async fn admitting(&self, incoming: u64) -> bool {
        if incoming > self.max_bytes {
            self.gave_up::<()>(TOO_BIG);
            return false;
        }

        let Some(blobs) = self.listed().await else {
            return false;
        };

        let mut total: u64 = blobs.iter().map(|blob| blob.size).sum();
        if total + incoming <= self.max_bytes {
            return true;
        }

        for entry in oldest_first(&blobs) {
            if total + incoming <= self.target_bytes {
                break;
            }

            if self.remove(&entry.blobs).await {
                total = total.saturating_sub(entry.bytes);
            }
        }

        total + incoming <= self.max_bytes
    }

    async fn listed(&self) -> Option<Vec<blob::Blob>> {
        let blobs = self.blobs()?;
        self.answered(blobs.list(&self.prefix).await, LISTING)
    }

    async fn remove(&self, pathnames: &[&str]) -> bool {
        let Some(blobs) = self.blobs() else {
            return false;
        };

        self.answered(blobs.delete(pathnames).await, DELETING)
            .is_some()
    }

    async fn holds(&self, blobs: &Blobs, pathname: &str) -> Presence {
        match self.answered(blobs.head(pathname).await, WRITING) {
            Some(true) => Presence::There,
            Some(false) => Presence::Missing,
            None => Presence::Unknown,
        }
    }

    async fn read(&self, pathname: &str) -> Option<Vec<u8>> {
        let blobs = self.blobs()?;
        self.answered(blobs.read(pathname).await, READING)?
    }

    async fn write(&self, blobs: &Blobs, pathname: &str, bytes: Vec<u8>) {
        let Presence::Missing = self.holds(blobs, pathname).await else {
            return;
        };

        self.answered(blobs.write(pathname, bytes).await, WRITING);
    }

    fn blobs(&self) -> Option<&Blobs> {
        match &self.source {
            Source::Blobs(blobs) => Some(blobs),
            Source::Unavailable(why) => self.gave_up(why),
        }
    }

    fn answered<T>(&self, answer: Result<T, Failure>, doing: &'static str) -> Option<T> {
        match answer {
            Ok(answer) => Some(answer),
            Err(_) => self.gave_up(doing),
        }
    }

    fn gave_up<T>(&self, doing: &'static str) -> Option<T> {
        self.log.note(&Note {
            seam: "store",
            doing,
        });
        None
    }
}

struct Listed<'a> {
    bytes: u64,

    uploaded_at: &'a str,

    blobs: Vec<&'a str>,
}

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

    entries.sort_by(|left, right| {
        (left.uploaded_at, &left.blobs).cmp(&(right.uploaded_at, &right.blobs))
    });

    entries
}

fn in_background(work: impl Future<Output = ()> + Send + 'static) {
    AppState::new(LogContext::new(None, None, None)).wait_until(work);
}

const NO_CREDENTIALS: &str = "reading the blob store's credentials";

const READING: &str = "reading a cached result from the blob store";

const WRITING: &str = "writing to the blob store";

const LISTING: &str = "listing what the blob store holds";

const DELETING: &str = "evicting an entry from the blob store";

const TOO_BIG: &str = "admitting an entry larger than the whole budget";

impl std::fmt::Debug for DiffStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let source = match &self.source {
            Source::Blobs(Blobs::Live(_)) => "live",
            Source::Blobs(Blobs::Memory(_)) => "memory",
            Source::Unavailable(why) => why,
        };

        f.debug_struct("DiffStore")
            .field("source", &source)
            .finish()
    }
}

#[derive(Debug, Clone, Default)]
pub struct Memory {
    blobs: Arc<Mutex<BTreeMap<String, Held>>>,

    stall: Duration,

    failing: Option<Failing>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Operation {
    List,

    Head,

    Read,

    Write,

    Delete,
}

#[derive(Debug, Clone)]
struct Failing {
    operation: Operation,

    asked: Arc<AtomicUsize>,

    fails: Range<usize>,
}

#[derive(Debug, Clone)]
struct Held {
    bytes: Vec<u8>,
    uploaded_at: String,
}

const STAGED: Failure = Failure::Internal {
    doing: "failing an operation this store was told to fail",
};

static UPLOADS: AtomicU64 = AtomicU64::new(0);

impl Memory {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn stalling(self, delay: Duration) -> Self {
        Self {
            stall: delay,
            ..self
        }
    }

    pub fn failing(store: &Self, operation: Operation) -> Self {
        Self::failing_for(store, operation, 0..usize::MAX)
    }

    pub fn failing_after(store: &Self, operation: Operation, kept: usize) -> Self {
        Self::failing_for(store, operation, kept..usize::MAX)
    }

    pub fn failing_once(store: &Self, operation: Operation) -> Self {
        Self::failing_for(store, operation, 0..1)
    }

    fn failing_for(store: &Self, operation: Operation, fails: Range<usize>) -> Self {
        Self {
            failing: Some(Failing {
                operation,
                asked: Arc::new(AtomicUsize::new(0)),
                fails,
            }),
            ..store.clone()
        }
    }

    pub fn uploaded_at(&self, pathname: &str) -> Option<String> {
        Some(self.blobs.lock().ok()?.get(pathname)?.uploaded_at.clone())
    }

    pub fn blob(&self, pathname: &str) -> Option<Vec<u8>> {
        Some(self.blobs.lock().ok()?.get(pathname)?.bytes.clone())
    }

    pub fn forget(&self, pathname: &str) {
        if let Ok(mut blobs) = self.blobs.lock() {
            blobs.remove(pathname);
        }
    }

    pub fn written(&self) -> Vec<String> {
        self.blobs
            .lock()
            .map(|blobs| blobs.keys().cloned().collect())
            .unwrap_or_default()
    }

    pub fn store(&self) -> DiffStore {
        DiffStore::over(Source::Blobs(Blobs::Memory(self.clone())))
    }

    async fn read(&self, pathname: &str) -> Result<Option<Vec<u8>>, Failure> {
        self.refusing(Operation::Read)?;

        Ok(self.blob(pathname))
    }

    async fn list(&self, prefix: &str) -> Result<Vec<blob::Blob>, Failure> {
        self.stalled().await;
        self.refusing(Operation::List)?;

        let Ok(blobs) = self.blobs.lock() else {
            return Ok(Vec::new());
        };

        Ok(blobs
            .iter()
            .filter(|(pathname, _)| pathname.starts_with(prefix))
            .map(|(pathname, held)| blob::Blob {
                pathname: pathname.clone(),
                size: held.bytes.len() as u64,
                uploaded_at: held.uploaded_at.clone(),
            })
            .collect())
    }

    async fn delete(&self, pathnames: &[&str]) -> Result<(), Failure> {
        self.stalled().await;
        self.refusing(Operation::Delete)?;

        if let Ok(mut blobs) = self.blobs.lock() {
            for pathname in pathnames {
                blobs.remove(*pathname);
            }
        }

        Ok(())
    }

    fn refusing(&self, operation: Operation) -> Result<(), Failure> {
        let Some(failing) = self.failing.as_ref().filter(|f| f.operation == operation) else {
            return Ok(());
        };

        let nth = failing.asked.fetch_add(1, Ordering::Relaxed);
        match failing.fails.contains(&nth) {
            true => Err(STAGED),
            false => Ok(()),
        }
    }

    async fn stalled(&self) {
        if !self.stall.is_zero() {
            tokio::time::sleep(self.stall).await;
        }
    }

    async fn head(&self, pathname: &str) -> Result<bool, Failure> {
        self.refusing(Operation::Head)?;

        Ok(self
            .blobs
            .lock()
            .is_ok_and(|blobs| blobs.contains_key(pathname)))
    }

    async fn write(&self, pathname: &str, bytes: Vec<u8>) -> Result<(), Failure> {
        self.stalled().await;
        self.refusing(Operation::Write)?;

        if let Ok(mut blobs) = self.blobs.lock() {
            let uploaded_at = format!("{:020}", UPLOADS.fetch_add(1, Ordering::Relaxed));
            blobs.insert(pathname.to_owned(), Held { bytes, uploaded_at });
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, SystemTime, UNIX_EPOCH};

    use super::*;

    #[tokio::test]
    #[ignore = "networked: writes to this project's Vercel Blob store"]
    async fn a_sweep_against_the_real_store_leaves_the_newest_entry() {
        let credentials = blob::Credentials::from_env()
            .expect("VERCEL_OIDC_TOKEN with BLOB_STORE_ID, or BLOB_READ_WRITE_TOKEN");
        let seeding = blob::Api::live(credentials);

        let run = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("the clock is set after 1970")
            .as_nanos();

        let prefix = format!("tests/sweep-{run}/");

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

        let credentials = blob::Credentials::from_env().expect("the same two variables");
        let store = DiffStore::over(Source::Blobs(Blobs::Live(blob::Api::live(credentials))))
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
