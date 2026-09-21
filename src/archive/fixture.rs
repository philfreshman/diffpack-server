//! Archives from a directory rather than from a registry.
//!
//! `index.json` in that directory maps a URL to the file that stands in for
//! what it serves, so this adapter answers the same question the live one
//! does — *what is at this URL* — and differs only in where it looks. A URL
//! the index does not carry is a gap in the fixture set rather than
//! something a model can act on, which is why it takes the internal channel.
//!
//! # A URL that serves nothing
//!
//! `null` in the index is a third answer, and a real one: the registry has
//! no such package or no such version. It is distinct from a URL the index
//! never mentions — that is a hole in the fixture set and nobody's to act
//! on — and it is what lets the offline suite drive the path a `404` takes
//! without a `404`. The refusal it produces is built by
//! [`super::not_found`], which is the live adapter's too, so the two cannot
//! answer differently.

use std::collections::HashMap;
use std::path::PathBuf;

use crate::error::Failure;
use crate::registry::Registry;

/// The archives under one directory.
#[derive(Debug)]
pub struct Fixture {
    dir: PathBuf,
}

impl Fixture {
    pub fn new(dir: PathBuf) -> Self {
        Self { dir }
    }

    /// The bytes the fixture set serves for `url`.
    ///
    /// `registry`, `package` and `version` are here for the refusal rather
    /// than for the lookup, exactly as they are in the live adapter: a
    /// failure a model can act on says what was not found, and this is the
    /// only place that knows both the URL and what was asked for.
    pub fn bytes(
        &self,
        url: &str,
        registry: Registry,
        package: &str,
        version: &str,
    ) -> Result<Vec<u8>, Failure> {
        let missing = Failure::Internal {
            doing: "reading the archive fixtures",
        };

        let index = std::fs::read(self.dir.join("index.json")).map_err(|_| Failure::Internal {
            doing: "reading the archive fixtures",
        })?;
        let index: HashMap<String, Option<String>> =
            serde_json::from_slice(&index).map_err(|_| Failure::Internal {
                doing: "reading the archive fixtures",
            })?;

        let file = match index.get(url) {
            Some(Some(file)) => file,
            // The index says this URL serves nothing, which is what a
            // registry answers for a package or a version it does not have.
            Some(None) => return Err(super::not_found(registry, package, version)),
            None => return Err(missing),
        };

        std::fs::read(self.dir.join(file)).map_err(|_| Failure::Internal {
            doing: "reading the archive fixtures",
        })
    }
}
