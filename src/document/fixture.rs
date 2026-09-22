//! Documents from a directory rather than from a registry.
//!
//! `index.json` in that directory maps a URL to the file that stands in for
//! what it serves, so this adapter answers the same question the live one
//! does — *what is at this URL* — and differs only in where it looks. One
//! format and one reader, where there used to be three of each: the archive,
//! version and search sets were already written the same way on disk, and the
//! three readers of them had started to differ in nothing that mattered and
//! everything that is easy to get subtly wrong.
//!
//! A URL the index does not carry is a gap in the fixture set rather than
//! something a model can act on, which is why it takes the internal channel,
//! and why the set that was short is named in it.
//!
//! # A URL that serves nothing
//!
//! `null` in the index is a third answer, and a real one: the registry serves
//! nothing there. It is distinct from a URL the index never mentions — that
//! is a hole in the fixture set and nobody's to act on — and it is what lets
//! an offline suite drive the path a refusal takes without a refusal. The
//! refusal it produces is the live adapter's own, built by
//! [`super::About::missing`] from the status
//! [`super::About::nothing_there`] names, so a fixture set and a registry
//! cannot disagree about what a missing thing reads like.

use std::collections::HashMap;
use std::path::PathBuf;

use crate::error::Failure;

use super::{About, Body};

/// The documents under one directory.
#[derive(Debug)]
pub struct Fixture {
    dir: PathBuf,
    /// What an operator is told this adapter was doing when the set came up
    /// short. It names which of the sets it was, because "a fixture is
    /// missing" is only actionable with that word in it.
    doing: &'static str,
}

impl Fixture {
    pub fn new(dir: PathBuf, doing: &'static str) -> Self {
        Self { dir, doing }
    }

    /// Whatever the fixture set serves for `url`, refusing anything over
    /// `limit`.
    ///
    /// The only place a fixture body is weighed, and the reason the cap is a
    /// rule about what this server will read rather than about where bytes
    /// came from: nothing streamed these, so a set that answered with
    /// something the registries would have been refused for would otherwise
    /// go through.
    pub fn body(&self, url: &str, limit: u64, about: &About<'_>) -> Result<Body, Failure> {
        let short = || Failure::Internal { doing: self.doing };

        let index = std::fs::read(self.dir.join("index.json")).map_err(|_| short())?;
        let index: HashMap<String, Option<String>> =
            serde_json::from_slice(&index).map_err(|_| short())?;

        let file = match index.get(url) {
            Some(Some(file)) => file,
            Some(None) => return Err((about.missing)(about.nothing_there)),
            None => return Err(short()),
        };

        let bytes = std::fs::read(self.dir.join(file)).map_err(|_| short())?;

        let weight = bytes.len() as u64;
        if weight > limit {
            return Err((about.too_large)(weight));
        }
        Ok(Body::Owned(bytes))
    }
}
