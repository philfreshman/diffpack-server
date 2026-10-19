use std::future::Future;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use rmcp::model::JsonObject;
use serde::Serialize;
use serde_json::Value;

use crate::error::Failure;

#[derive(Debug, Serialize)]
pub struct Line {
    #[serde(flatten)]
    pub subject: Subject,

    pub args: Arguments,

    pub result: &'static str,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub cache: Option<&'static str>,

    pub ms: Phases,
}

impl Line {
    pub fn calling(tool: &str) -> Self {
        Self::naming(Subject::Tool(shorten(tool)))
    }

    pub fn reading(resource: &str) -> Self {
        Self::naming(Subject::Resource(shorten(resource)))
    }

    pub fn serving(route: &str) -> Self {
        Self::naming(Subject::Route(shorten(route)))
    }

    fn naming(subject: Subject) -> Self {
        Self {
            subject,
            args: Arguments::default(),
            result: "ok",
            cache: None,
            ms: Phases::default(),
        }
    }

    pub fn about(self, arguments: Option<&JsonObject>) -> Self {
        Self {
            args: Arguments::of(arguments),
            ..self
        }
    }

    pub fn taking(self, total: Duration, spent: &Spent) -> Self {
        Self {
            ms: Phases {
                total: millis(total),
                fetch: spent.fetch().map(millis),
            },
            ..self
        }
    }

    pub fn cached(self, lookup: &Lookup) -> Self {
        Self {
            cache: lookup.outcome(),
            ..self
        }
    }

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

#[derive(Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Subject {
    Tool(String),

    Resource(String),

    Route(String),
}

impl Subject {
    pub fn doing(&self) -> &'static str {
        match self {
            Self::Tool(_) => "calling a tool",
            Self::Resource(_) => "reading a resource",
            Self::Route(_) => "serving a route",
        }
    }
}

#[derive(Debug, Default, Serialize)]
pub struct Phases {
    pub total: f64,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub fetch: Option<f64>,
}

#[derive(Debug)]
pub struct Spent {
    started: Instant,

    fetched: AtomicBool,

    first: AtomicU64,

    last: AtomicU64,
}

impl Spent {
    pub fn new() -> Self {
        Self::starting_at(Instant::now())
    }

    pub fn starting_at(started: Instant) -> Self {
        Self {
            started,
            fetched: AtomicBool::new(false),
            first: AtomicU64::new(u64::MAX),
            last: AtomicU64::new(0),
        }
    }

    pub fn fetching(&self, began: Instant, ended: Instant) {
        self.fetched.store(true, Ordering::Relaxed);
        self.first.fetch_min(self.offset(began), Ordering::Relaxed);
        self.last.fetch_max(self.offset(ended), Ordering::Relaxed);
    }

    pub async fn while_fetching<T>(&self, work: impl Future<Output = T>) -> T {
        let _window = FetchWindow {
            spent: self,
            began: Instant::now(),
        };

        work.await
    }

    pub fn fetch(&self) -> Option<Duration> {
        self.fetched.load(Ordering::Relaxed).then(|| {
            let first = self.first.load(Ordering::Relaxed);
            let last = self.last.load(Ordering::Relaxed);
            Duration::from_micros(last.saturating_sub(first))
        })
    }

    fn offset(&self, at: Instant) -> u64 {
        u64::try_from(at.saturating_duration_since(self.started).as_micros()).unwrap_or(u64::MAX)
    }
}

struct FetchWindow<'a> {
    spent: &'a Spent,

    began: Instant,
}

impl Drop for FetchWindow<'_> {
    fn drop(&mut self) {
        self.spent.fetching(self.began, Instant::now());
    }
}

impl Default for Spent {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Default)]
pub struct Lookup {
    asked: AtomicBool,

    found: AtomicBool,

    storeless: AtomicBool,
}

impl Lookup {
    pub fn looked(&self, found: bool) {
        self.asked.store(true, Ordering::Relaxed);
        self.found.fetch_or(found, Ordering::Relaxed);
    }

    pub fn found_no_store(&self) {
        self.asked.store(true, Ordering::Relaxed);
        self.storeless.store(true, Ordering::Relaxed);
    }

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

fn millis(duration: Duration) -> f64 {
    (duration.as_secs_f64() * 10_000.0).round() / 10.0
}

#[derive(Debug, Default, Serialize)]
#[serde(transparent)]
pub struct Arguments(JsonObject);

impl Arguments {
    const LONGEST: usize = 100;

    const CUT: char = '…';

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

fn summarise(value: &Value) -> Value {
    match value.as_str() {
        Some(text) => Value::String(shorten(text)),
        None => value.clone(),
    }
}

fn shorten(text: &str) -> String {
    let text = crate::error::redact(text);

    let kept: String = text.chars().take(Arguments::LONGEST).collect();
    if kept.len() == text.len() {
        return kept;
    }

    format!("{kept}{}", Arguments::CUT)
}

#[derive(Debug, Serialize)]
pub struct Note {
    pub seam: &'static str,

    pub doing: &'static str,
}

#[derive(Debug, Clone, Default)]
pub enum Sink {
    #[default]
    Stderr,

    Captured(Arc<Mutex<Vec<String>>>),
}

impl Sink {
    pub fn write(&self, line: &Line) {
        self.emit(serde_json::to_string(line));
    }

    pub fn note(&self, note: &Note) {
        self.emit(serde_json::to_string(note));
    }

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

#[derive(Debug, Clone, Default)]
pub struct Capture(Arc<Mutex<Vec<String>>>);

impl Capture {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn sink(&self) -> Sink {
        Sink::Captured(Arc::clone(&self.0))
    }

    pub fn lines(&self) -> Vec<String> {
        self.0
            .lock()
            .expect("the capturing sink's lock should not be poisoned")
            .clone()
    }
}
