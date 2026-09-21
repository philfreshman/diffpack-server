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
//! [`Record`] is built, then written. That is what lets the suite assert what
//! a real call emitted: [`Sink`] has a variant that keeps lines in memory, a
//! [`Ctx`](crate::tools::Ctx) carries one, and a test reads back the line the
//! call produced rather than a line a test built. Emission that went straight
//! to stderr would leave "every tool call emits one line" as something nobody
//! could check.

use std::sync::{Arc, Mutex};

use rmcp::model::JsonObject;
use serde::Serialize;
use serde_json::Value;

use crate::error::Failure;

/// One tool call, as the line it leaves behind.
#[derive(Debug, Serialize)]
pub struct Record {
    /// Which tool ran.
    pub tool: String,

    /// What it was asked for.
    pub args: Summary,

    /// How the call ended.
    pub result: &'static str,
}

impl Record {
    /// The line for a call of `tool`.
    pub fn new(tool: &str) -> Self {
        Self {
            tool: tool.to_owned(),
            args: Summary::default(),
            result: "ok",
        }
    }

    /// The same line, carrying what the call was for.
    pub fn about(self, arguments: Option<&JsonObject>) -> Self {
        Self {
            args: Summary::of(arguments),
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

/// A call's arguments, as much of them as belongs in a line.
///
/// The arguments as they arrived rather than the fields this module thought
/// worth keeping. Which argument matters is the tool's business and there
/// will be nineteen tools; a list here would be a list to widen, and the
/// argument nobody thought to log is the one an incident turns on.
#[derive(Debug, Default, Serialize)]
#[serde(transparent)]
pub struct Summary(JsonObject);

impl Summary {
    /// The most of one argument's value a line carries.
    ///
    /// Sized so that the arguments a person reads arrive whole — a scoped
    /// package name and a version are nowhere near it — and the ones nobody
    /// reads do not. A cursor and a diff handle are the long ones, and their
    /// first characters tell two calls apart, which is all a line needs them
    /// for.
    const VALUE_CAP: usize = 100;

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
                .map(|(name, value)| (name.clone(), summarise(value)))
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
///
/// Redacted before it is cut, and the order is the whole of it. A signed URL
/// cut at a hundred characters can lose its `?` and arrive as an ordinary
/// URL with a token in the path, which is the shape the redactor no longer
/// recognises — so cutting first would hide a secret from the check rather
/// than from the line.
fn summarise(value: &Value) -> Value {
    let Some(text) = value.as_str() else {
        return value.clone();
    };

    // `redact` is `crate::error`'s, so what must not reach an operator and
    // what must not reach a model are one definition. A second one here
    // would be a second list to widen when #20 brings a new credential
    // shape.
    let text = crate::error::redact(text);

    if text.len() <= Summary::VALUE_CAP {
        return Value::String(text);
    }

    // By characters rather than bytes: `VALUE_CAP` is about how much of a
    // line this takes up, and cutting a UTF-8 sequence in half would produce
    // a line that is not valid JSON.
    let kept: String = text.chars().take(Summary::VALUE_CAP).collect();
    Value::String(format!("{kept}{}", Summary::CUT))
}

/// Where a [`Record`] goes.
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
    /// Write `record` as one line.
    ///
    /// Every failure here is swallowed, deliberately: a request that was
    /// answered correctly must not fail because the line describing it could
    /// not be written. A log that can break the thing it observes is worse
    /// than no log.
    pub fn write(&self, record: &Record) {
        let Ok(line) = serde_json::to_string(record) else {
            return;
        };

        match self {
            Self::Stderr => eprintln!("{line}"),
            Self::Captured(lines) => {
                if let Ok(mut lines) = lines.lock() {
                    lines.push(line);
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
