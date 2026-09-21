//! Which packages a registry has, read from a directory.
//!
//! `index.json` maps a URL to the file that stands in for what it serves, so
//! this adapter answers the same question the live one does — *what is at
//! this URL* — and differs only in where it looks. A URL the index does not
//! carry is a gap in the fixture set rather than something a model can act
//! on, which is why it takes the internal channel.
//!
//! # A source that is not answering
//!
//! `null` in the index is a third answer and a real one: the source is down.
//! It is distinct from a URL the index never mentions — that is a hole in the
//! fixture set and nobody's to act on — and it is what lets the offline suite
//! drive the path a `503` takes without a `503`.

use std::collections::HashMap;
use std::path::PathBuf;

use crate::error::Failure;
use crate::registry::Registry;

use super::Body;

/// What a source that is not answering answers with, where it answers at all.
///
/// The number a fixture stands in with, so that the refusal the suite drives
/// is the one a real outage produces rather than one shaped like it.
const SOURCE_DOWN: u16 = 503;

/// The answers under one directory.
#[derive(Debug)]
pub struct Fixture {
    dir: PathBuf,
}

impl Fixture {
    pub fn new(dir: PathBuf) -> Self {
        Self { dir }
    }

    /// The body the fixture set serves for `url`.
    ///
    /// `registry` is here for the refusal rather than for the lookup, as it
    /// is in the archive fixtures: a failure a model can act on says which
    /// source went quiet, and this is the only place that knows both the URL
    /// and who it belongs to.
    pub fn body(&self, url: &str, registry: Registry) -> Result<Body, Failure> {
        let missing = || Failure::Internal {
            doing: "reading the search fixtures",
        };

        let index = std::fs::read(self.dir.join("index.json")).map_err(|_| missing())?;
        let index: HashMap<String, Option<String>> =
            serde_json::from_slice(&index).map_err(|_| missing())?;

        let file = match index.get(url) {
            Some(Some(file)) => file,
            // The index says this source is not answering, which is what a
            // registry having a bad day looks like from here.
            Some(None) => return Err(super::unavailable(registry, SOURCE_DOWN)),
            None => return Err(missing()),
        };
        std::fs::read_to_string(self.dir.join(file))
            .map(Into::into)
            .map_err(|_| missing())
    }
}
