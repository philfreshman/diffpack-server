//! Archives from a directory rather than from a registry.
//!
//! `index.json` in that directory maps a URL to the file that stands in for
//! what it serves, so this adapter answers the same question the live one
//! does — *what is at this URL* — and differs only in where it looks. A URL
//! the index does not carry is a gap in the fixture set rather than
//! something a model can act on, which is why it takes the internal channel.

use std::collections::HashMap;
use std::path::PathBuf;

use crate::error::Failure;

/// The archives under one directory.
pub struct Fixture {
    dir: PathBuf,
}

impl Fixture {
    pub fn new(dir: PathBuf) -> Self {
        Self { dir }
    }

    /// The bytes the fixture set serves for `url`.
    pub fn bytes(&self, url: &str) -> Result<Vec<u8>, Failure> {
        let index = std::fs::read(self.dir.join("index.json")).map_err(|_| Failure::Internal {
            doing: "reading the archive fixtures",
        })?;
        let index: HashMap<String, String> =
            serde_json::from_slice(&index).map_err(|_| Failure::Internal {
                doing: "reading the archive fixtures",
            })?;

        let file = index.get(url).ok_or(Failure::Internal {
            doing: "reading the archive fixtures",
        })?;

        std::fs::read(self.dir.join(file)).map_err(|_| Failure::Internal {
            doing: "reading the archive fixtures",
        })
    }
}
