//! What a registry publishes, read from a directory.
//!
//! `index.json` maps a URL to the file that stands in for what it serves, so
//! this adapter answers the same question the live one does — *what is at
//! this URL* — and differs only in where it looks. A URL the index does not
//! carry is a gap in the fixture set rather than something a model can act
//! on, which is why it takes the internal channel.

use std::collections::HashMap;
use std::path::PathBuf;

use crate::error::Failure;

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
    pub fn body(&self, url: &str) -> Result<String, Failure> {
        let missing = || Failure::Internal {
            doing: "reading the search fixtures",
        };

        let index = std::fs::read(self.dir.join("index.json")).map_err(|_| missing())?;
        let index: HashMap<String, String> =
            serde_json::from_slice(&index).map_err(|_| missing())?;

        let file = index.get(url).ok_or_else(missing)?;
        std::fs::read_to_string(self.dir.join(file)).map_err(|_| missing())
    }
}
