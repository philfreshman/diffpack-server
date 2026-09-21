//! Version lists from a directory rather than from a registry.
//!
//! `index.json` in that directory maps a URL to the file that stands in for
//! what it serves, so this adapter answers the same question the live one
//! does — *what is at this URL* — and differs only in where it looks. A URL
//! the index does not carry is a gap in the fixture set rather than something
//! a model can act on, which is why it takes the internal channel.
//!
//! `null` in the index is a third answer and a real one: the registry has no
//! such package. It is what lets the offline suite drive the path a `404`
//! takes without a `404`, and the refusal it produces is built by
//! [`super::no_such_package`], which is the live adapter's too.

use std::collections::HashMap;
use std::path::PathBuf;

use crate::error::Failure;
use crate::registry::Registry;

/// The version documents under one directory.
#[derive(Debug)]
pub struct Fixture {
    dir: PathBuf,
}

impl Fixture {
    pub fn new(dir: PathBuf) -> Self {
        Self { dir }
    }

    /// The bytes the fixture set serves for `url`.
    pub fn bytes(&self, url: &str, registry: Registry, package: &str) -> Result<Vec<u8>, Failure> {
        let missing = Failure::Internal {
            doing: "reading the version fixtures",
        };

        let index = std::fs::read(self.dir.join("index.json")).map_err(|_| Failure::Internal {
            doing: "reading the version fixtures",
        })?;
        let index: HashMap<String, Option<String>> =
            serde_json::from_slice(&index).map_err(|_| Failure::Internal {
                doing: "reading the version fixtures",
            })?;

        let file = match index.get(url) {
            Some(Some(file)) => file,
            // The index says this URL serves nothing, which is what a
            // registry answers for a package it does not have.
            Some(None) => return Err(super::no_such_package(registry, package)),
            None => return Err(missing),
        };

        std::fs::read(self.dir.join(file)).map_err(|_| Failure::Internal {
            doing: "reading the version fixtures",
        })
    }
}
