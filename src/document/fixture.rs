use std::collections::HashMap;
use std::path::PathBuf;

use crate::error::Failure;

use super::{About, Body};

#[derive(Debug)]
pub struct Fixture {
    dir: PathBuf,
    doing: &'static str,
}

impl Fixture {
    pub fn new(dir: PathBuf, doing: &'static str) -> Self {
        Self { dir, doing }
    }

    pub fn body(&self, url: &str, about: &About<'_>) -> Result<Body, Failure> {
        let short = || Failure::Internal { doing: self.doing };

        let index = std::fs::read(self.dir.join("index.json")).map_err(|_| short())?;
        let index: HashMap<String, Option<String>> =
            serde_json::from_slice(&index).map_err(|_| short())?;

        let file = match index.get(url) {
            Some(Some(file)) => file,
            Some(None) => return Err((about.missing)(about.nothing_there)),
            None => return Err(short()),
        };

        std::fs::read(self.dir.join(file))
            .map(Body::Owned)
            .map_err(|_| short())
    }
}
