//! One line per tool call.
//!
//! What an incident is read from. The questions asked when this server has
//! misbehaved are always the same ones — which tool ran, what was it asked
//! for, how long did it take, how did it end — so the answer is one
//! structured line per call rather than prose scattered through the handlers
//! that happened to want it.
//!
//! # Why the line is a value before it is output
//!
//! [`Line`] is built, then written. That is what lets the suite assert what
//! a real call emitted: [`Sink`] has a variant that keeps lines in memory, a
//! [`Ctx`](crate::tools::Ctx) carries one, and a test reads back the line the
//! call produced rather than a line a test built. Emission that went straight
//! to stderr would leave "every tool call emits one line" as something nobody
//! could check.

use std::future::Future;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use rmcp::model::JsonObject;
use serde::Serialize;
use serde_json::Value;

use crate::error::Failure;

/// One tool call, as the line it leaves behind.
///
/// Named for what `CONTEXT.md` calls it. The glossary coins Line, Phase and
/// Cause in the same change this module arrives in, and a type here called
/// anything else would be the drift those entries exist to stop.
#[derive(Debug, Serialize)]
pub struct Line {
    /// Which tool ran.
    pub tool: String,

    /// What it was asked for.
    pub args: Arguments,

    /// How the call ended.
    pub result: &'static str,

    /// Whether the answer was remembered or worked out, where the call asked
    /// the store at all — or that there was no store to ask.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cache: Option<&'static str>,

    /// How long it took, by phase.
    pub ms: Phases,
}

impl Line {
    /// The line for a call of `tool`.
    ///
    /// Shortened like an argument is, because a name that reached here is not
    /// always one of ours: a name no tool answers to is refused by the
    /// dispatch, and whatever the caller sent is what this line is about.
    pub fn new(tool: &str) -> Self {
        Self {
            tool: shorten(tool),
            args: Arguments::default(),
            result: "ok",
            cache: None,
            ms: Phases::default(),
        }
    }

    /// The same line, carrying what the call was for.
    pub fn about(self, arguments: Option<&JsonObject>) -> Self {
        Self {
            args: Arguments::of(arguments),
            ..self
        }
    }

    /// The same line, saying where the call's time went.
    pub fn taking(self, total: Duration, spent: &Spent) -> Self {
        Self {
            ms: Phases {
                total: millis(total),
                fetch: spent.fetch().map(millis),
            },
            ..self
        }
    }

    /// The same line, saying whether the answer was remembered.
    ///
    /// The same fact `diff_package_versions` answers with, for the other
    /// reader: an agent is told so that it knows asking twice is cheap, and
    /// an operator is told so that a percentile over one tool's calls is not
    /// taken over two populations at once — a hit is a lookup, and a miss is
    /// two archive downloads and a tree built out of them.
    pub fn cached(self, lookup: &Lookup) -> Self {
        Self {
            cache: lookup.outcome(),
            ..self
        }
    }

    /// The same line, saying how the call ended.
    pub fn ending<T>(self, answer: &Result<T, Failure>) -> Self {
        Self {
            result: match answer {
                Ok(_) => "ok",
                Err(failure) => failure.kind(),
            },
            ..self
        }
    }
}

/// Where a call's time went, in milliseconds.
///
/// Milliseconds because that is the unit the rest of this deployment is
/// discussed in — [`UPSTREAM_TIMEOUT`](crate::error::UPSTREAM_TIMEOUT), the
/// function's own ceiling in `vercel.json` — and a number an operator has to
/// convert before comparing is one they will convert wrongly.
///
/// Two phases today, because two is what this tree can tell apart. The
/// download-extract-diff-store split #26 asks for needs each of those to be
/// something this crate can put a stopwatch on, and none of the four is yet.
/// Download and extract are one interface by [ADR
/// 0001](../docs/adr/0001-the-archive-seam-is-a-filemap.md), which exists so
/// a caller cannot see how many requests a fetch took; the diff is a
/// synchronous call inside [`crate::engine`], which under [ADR
/// 0007](../docs/adr/0007-one-importer-of-the-engine.md) has no reach into a
/// request; and there is no store to write to until #20. Each is a decision
/// about a seam rather than a field to add, so `total - fetch` is what the
/// rest of a call costs until one of them is made.
///
/// `fetch` is absent rather than zero when a tool fetched nothing. Zero is a
/// measurement, and a percentile taken over a column where half the rows are
/// a phase that never ran describes neither population.
#[derive(Debug, Default, Serialize)]
pub struct Phases {
    /// Everything: argument validation, the work, and building the answer.
    pub total: f64,

    /// The part of it spent waiting for a registry, where any was.
    ///
    /// The one split worth making before the others exist: a slow call is
    /// either a slow registry or slow work here, and those are somebody
    /// else's incident and ours. Every seam that goes out of this process
    /// counts towards it — an Archive and a Catalogue alike — because the
    /// question is about the wait and not about which document was waited
    /// for.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fetch: Option<f64>,
}

/// Where a request's time goes, while it is still being spent.
///
/// Shared across one request and written from wherever the phase actually
/// happens — the seam, rather than a timer threaded down through a handler
/// that would then have to pass it back up. Atomics rather than a lock
/// because nothing here ever reads a value it is about to write, so there is
/// no invariant a lock would be protecting.
///
/// # Why a window and not a sum
///
/// `diff_package_versions` asks for two versions through one `try_join!`, so
/// two fetches overlap. Summing their durations reports a thousand
/// milliseconds where the call waited five hundred, in the field directly
/// beside [`Phases::total`] — which a reader compares it against, and which
/// it can then exceed.
///
/// So what is kept is the window: the earliest a fetch began to the latest
/// one ended. That is the question this number is next to `total` to answer —
/// how much of the call went on waiting for a registry — and it makes
/// `fetch <= total` true rather than usually true.
///
/// What it is not is how much registry work the call caused, which is the
/// summed figure and is a different question. Nothing asks it yet; when
/// something does it goes beside this rather than replacing it.
#[derive(Debug)]
pub struct Spent {
    /// When the request started. Every offset below is from here, which is
    /// what lets two of them be compared across threads without an `Instant`
    /// in an atomic.
    started: Instant,

    /// Whether any fetch happened at all, which is what tells "nothing was
    /// asked of a registry" from "asking was instant".
    fetched: AtomicBool,

    /// Microseconds from [`Self::started`] to the earliest fetch beginning.
    first: AtomicU64,

    /// Microseconds from [`Self::started`] to the latest fetch ending.
    last: AtomicU64,
}

impl Spent {
    /// A fresh tally for a request starting now.
    pub fn new() -> Self {
        Self::starting_at(Instant::now())
    }

    /// The same, for a request that started at `started`.
    ///
    /// The seam `tests/log.rs` drives to put two overlapping fetches in by
    /// hand. It is the one thing about this module a call over the wire
    /// cannot show: the fixture archives answer in under a millisecond, so no
    /// call the suite can make overlaps enough for a window and a sum to
    /// differ.
    pub fn starting_at(started: Instant) -> Self {
        Self {
            started,
            fetched: AtomicBool::new(false),
            first: AtomicU64::new(u64::MAX),
            last: AtomicU64::new(0),
        }
    }

    /// Record a fetch that ran from `began` to `ended`.
    pub fn fetching(&self, began: Instant, ended: Instant) {
        self.fetched.store(true, Ordering::Relaxed);
        self.first.fetch_min(self.offset(began), Ordering::Relaxed);
        self.last.fetch_max(self.offset(ended), Ordering::Relaxed);
    }

    /// Run `work`, and put the window it took on this request's fetch phase.
    ///
    /// What a seam calls, rather than reading the two instants itself. There
    /// is one seam per thing this server fetches and there will be more, and
    /// a phase each of them times its own way is a phase that means something
    /// slightly different per row.
    ///
    /// Recorded whether or not `work` succeeded. A registry that times out is
    /// exactly the call worth knowing the wait for.
    pub async fn while_fetching<T>(&self, work: impl Future<Output = T>) -> T {
        let began = Instant::now();
        let done = work.await;

        self.fetching(began, Instant::now());
        done
    }

    /// How long the call spent waiting for a registry, or nothing if it
    /// asked one for nothing.
    pub fn fetch(&self) -> Option<Duration> {
        self.fetched.load(Ordering::Relaxed).then(|| {
            let first = self.first.load(Ordering::Relaxed);
            let last = self.last.load(Ordering::Relaxed);
            Duration::from_micros(last.saturating_sub(first))
        })
    }

    /// `at`, as microseconds since this request started.
    ///
    /// Saturating rather than panicking on an instant before the start: a
    /// clock question is not worth failing a request that otherwise worked,
    /// and a zero here reads as "from the beginning", which is what it would
    /// mean.
    fn offset(&self, at: Instant) -> u64 {
        u64::try_from(at.saturating_duration_since(self.started).as_micros()).unwrap_or(u64::MAX)
    }
}

impl Default for Spent {
    fn default() -> Self {
        Self::new()
    }
}

/// What a call's lookup in the store found, while the call is still running.
///
/// The cache's half of what [`Spent`] does for time, and shared the same way:
/// written where the lookup happens rather than reported by the handler that
/// made it, so that a tool cannot answer out of the store without the line
/// saying so.
///
/// One value per call and not one per lookup. A call asks once today, and a
/// call that asked twice was either served or not — so a hit wins over a
/// miss, which is the reading an operator makes of a call that avoided the
/// downloads.
#[derive(Debug, Default)]
pub struct Lookup {
    /// Whether the store was asked at all, which is what tells a tool that
    /// reads no cache from one that read a cold one.
    asked: AtomicBool,

    /// Whether any lookup found its entry.
    found: AtomicBool,

    /// Whether the store a lookup was made in is a store at all.
    ///
    /// A deployment fact and not a lookup's: it is settled when the store is
    /// built, by whether there were credentials to build a client from, and
    /// is the same answer for every call this instance serves. Kept here
    /// rather than read off the store when the line is written because the
    /// line is written by the dispatch, which has a `Ctx` and not a store —
    /// and putting it where the other two are keeps one rule for how this
    /// field is filled in.
    storeless: AtomicBool,
}

impl Lookup {
    /// Record a lookup that found an entry, or did not.
    pub fn looked(&self, found: bool) {
        self.asked.store(true, Ordering::Relaxed);
        self.found.fetch_or(found, Ordering::Relaxed);
    }

    /// Record a lookup that had no store to make.
    ///
    /// Beside [`Lookup::looked`] rather than a third argument to it, because
    /// it is not a third thing a lookup found: nothing was looked in. A
    /// store that is not there answers `None` to every `get`, so a call that
    /// went through `looked` would be indistinguishable from a cold cache —
    /// which is the whole of what this records.
    pub fn found_no_store(&self) {
        self.asked.store(true, Ordering::Relaxed);
        self.storeless.store(true, Ordering::Relaxed);
    }

    /// `hit`, `miss`, `no_store`, or nothing where the call never asked the
    /// store.
    ///
    /// Absent rather than `miss` for the reason [`Phases::fetch`] is absent
    /// rather than zero: a hit rate taken over a column where most rows are
    /// tools with no cache to hit describes neither the cache nor the tools.
    ///
    /// `no_store` is the same rule one level down, and is a value rather
    /// than a second kind of absence. A deployment with no credentials
    /// answers every lookup with nothing, so counted as a miss its hit rate
    /// is a flat hundred percent miss — which is what a cache that is
    /// working and cold reads as. Left absent instead it would be
    /// indistinguishable from a tool that never asks, so the field says
    /// which of the two it is looking at.
    ///
    /// A hit is checked first and cannot be reached storelessly: a store
    /// that is not there has nothing to serve.
    pub fn outcome(&self) -> Option<&'static str> {
        if !self.asked.load(Ordering::Relaxed) {
            return None;
        }

        if self.found.load(Ordering::Relaxed) {
            return Some("hit");
        }

        Some(match self.storeless.load(Ordering::Relaxed) {
            true => "no_store",
            false => "miss",
        })
    }
}

/// `duration` in milliseconds, to a tenth.
///
/// Rounded because the digits below it are noise — the same call on the same
/// input differs by more than that between invocations — and a line carrying
/// them invites a comparison that means nothing.
fn millis(duration: Duration) -> f64 {
    (duration.as_secs_f64() * 10_000.0).round() / 10.0
}

/// A call's arguments, as much of them as belongs in a line.
///
/// Not a Summary: that is the Totals and the sample `diff_package_versions`
/// answers with, and a second meaning for it here would cost the glossary the
/// one it already has.
///
/// The arguments as they arrived rather than the fields this module thought
/// worth keeping. Which argument matters is the tool's business and there
/// will be nineteen tools; a list here would be a list to widen, and the
/// argument nobody thought to log is the one an incident turns on.
#[derive(Debug, Default, Serialize)]
#[serde(transparent)]
pub struct Arguments(JsonObject);

impl Arguments {
    /// The most of one name or one value a line carries.
    ///
    /// Sized so that the arguments a person reads arrive whole — a scoped
    /// package name and a version are nowhere near it — and the ones nobody
    /// reads do not. A cursor and a diff handle are the long ones, and their
    /// first characters tell two calls apart, which is all a line needs them
    /// for.
    const LONGEST: usize = 100;

    /// What a cut value ends with, so that a shortened value cannot be read
    /// as a complete one.
    const CUT: char = '…';

    /// `arguments`, summarised.
    fn of(arguments: Option<&JsonObject>) -> Self {
        let Some(arguments) = arguments else {
            return Self::default();
        };

        Self(
            arguments
                .iter()
                .map(|(name, value)| (shorten(name), summarise(value)))
                .collect(),
        )
    }
}

/// One argument's value: redacted, then short enough to sit in a line.
///
/// Only strings are touched. A number, a boolean and a null are already short
/// and carry nothing to redact, and a structure is left to `serde_json` — no
/// tool takes one today, and guessing at how to shorten a shape nothing sends
/// would be guessing.
fn summarise(value: &Value) -> Value {
    match value.as_str() {
        Some(text) => Value::String(shorten(text)),
        None => value.clone(),
    }
}

/// One piece of caller-supplied text: redacted, then short enough to sit in a
/// line.
///
/// A name goes through this as well as a value, because both arrive from
/// whoever called: an argument nothing declared is summarised before any
/// schema has refused it, and a tool name nothing answers to is summarised
/// before the dispatch refuses it. A rule that covered only the values would
/// be a rule with the easier half of the line outside it.
///
/// Redacted before it is cut, and the order is the whole of it. A signed URL
/// cut at a hundred characters can lose its `?` and arrive as an ordinary
/// URL with a token in the path, which is the shape the redactor no longer
/// recognises — so cutting first would hide a secret from the check rather
/// than from the line.
fn shorten(text: &str) -> String {
    // `redact` is `crate::error`'s, so what must not reach an operator and
    // what must not reach a model are one definition. A second one here
    // would be a second list to widen when #20 brings a new credential
    // shape.
    let text = crate::error::redact(text);

    // Counted in characters rather than bytes, because cutting a UTF-8
    // sequence in half would produce a line that is not valid JSON — and
    // compared the same way, so that a value made of multi-byte characters
    // and left whole does not arrive wearing the mark of one that was cut.
    let kept: String = text.chars().take(Arguments::LONGEST).collect();
    if kept.len() == text.len() {
        return kept;
    }

    format!("{kept}{}", Arguments::CUT)
}

/// A seam saying it could not do its job, where that is not a failure.
///
/// The cache is the only one there is, and it is one by design: ADR 0003
/// says a cache failure must never fail a diff, so the failure has nowhere
/// else to go. A [`Failure`] would reach the model, and a [`Line`] is the
/// dispatch's — one per call, so that counting them counts calls — and is
/// already written by the time a backgrounded write has failed.
///
/// Two fields, because two is what an operator reading one needs: which seam
/// gave up, and what it was doing. What it did instead is the same thing
/// every time, and is this type's whole reason for existing.
#[derive(Debug, Serialize)]
pub struct Note {
    /// Which seam gave up. A [`Ctx`](crate::tools::Ctx)'s name for it.
    pub seam: &'static str,

    /// What it was doing, in the words a [`Failure`] would have used.
    pub doing: &'static str,
}

/// Where a [`Line`] goes.
///
/// Two variants rather than a trait, for the reason [ADR
/// 0004](../docs/adr/0004-one-registry-module.md) gives: neither can arrive
/// from outside this crate, so the extensibility a trait buys has no buyer.
#[derive(Debug, Clone, Default)]
pub enum Sink {
    /// Vercel's runtime logs, which is where a deployed function's stderr
    /// goes.
    #[default]
    Stderr,

    /// A buffer a test reads back. Built by [`Capture::sink`].
    Captured(Arc<Mutex<Vec<String>>>),
}

impl Sink {
    /// Write `line`.
    ///
    /// Every failure here is swallowed, deliberately: a request that was
    /// answered correctly must not fail because the line describing it could
    /// not be written. A log that can break the thing it observes is worse
    /// than no log.
    pub fn write(&self, line: &Line) {
        self.emit(serde_json::to_string(line));
    }

    /// Write `note`.
    ///
    /// Beside [`Sink::write`] rather than through it, because a [`Note`] is
    /// not a [`Line`] and must not be counted as one: a call that could not
    /// reach the cache is still one call.
    pub fn note(&self, note: &Note) {
        self.emit(serde_json::to_string(note));
    }

    /// Put one serialised record wherever this sink goes.
    ///
    /// Every failure here is swallowed for the reason above: a log that can
    /// break the thing it observes is worse than no log.
    fn emit(&self, record: Result<String, serde_json::Error>) {
        let Ok(record) = record else {
            return;
        };

        match self {
            Self::Stderr => eprintln!("{record}"),
            Self::Captured(lines) => {
                if let Ok(mut lines) = lines.lock() {
                    lines.push(record);
                }
            }
        }
    }
}

/// The lines a [`Sink`] collected, for a test to read back.
///
/// Separate from [`Sink`] so that reading is only possible where there is
/// something to read: a `lines()` on the stderr variant would answer "none"
/// to a test whose whole question was whether anything was written.
#[derive(Debug, Clone, Default)]
pub struct Capture(Arc<Mutex<Vec<String>>>);

impl Capture {
    pub fn new() -> Self {
        Self::default()
    }

    /// The sink that writes here, to hand to a [`Ctx`](crate::tools::Ctx).
    pub fn sink(&self) -> Sink {
        Sink::Captured(Arc::clone(&self.0))
    }

    /// Every line written so far, oldest first.
    pub fn lines(&self) -> Vec<String> {
        self.0
            .lock()
            .expect("the capturing sink's lock should not be poisoned")
            .clone()
    }
}
