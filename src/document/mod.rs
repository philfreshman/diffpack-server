mod fixture;
mod live;

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use crate::error::Failure;
use crate::registry::{self, Registry};

#[derive(Debug)]
pub struct Document {
    source: Source,
    limit: u64,
}

#[derive(Debug)]
enum Source {
    Live(live::Live),
    Fixture(fixture::Fixture),
}

pub struct About<'a> {
    pub registry: Registry,

    pub accept: Option<&'static str>,

    pub compressed: bool,

    pub remember_for: Option<Duration>,

    pub nothing_there: u16,

    pub blocked: &'a (dyn Fn() -> Failure + Send + Sync),

    pub missing: &'a (dyn Fn(u16) -> Failure + Send + Sync),

    pub too_large: &'a (dyn Fn(u64) -> Failure + Send + Sync),
}

impl Document {
    pub fn live(limit: u64) -> Self {
        Self {
            source: Source::Live(live::Live::new()),
            limit,
        }
    }

    pub fn fixture(dir: impl Into<PathBuf>, doing: &'static str, limit: u64) -> Self {
        Self {
            source: Source::Fixture(fixture::Fixture::new(dir.into(), doing)),
            limit,
        }
    }

    pub fn with_limit(self, limit: u64) -> Self {
        Self { limit, ..self }
    }

    pub fn limit(&self) -> u64 {
        self.limit
    }

    pub async fn body(&self, url: &str, about: &About<'_>) -> Result<Body, Failure> {
        if !registry::allows(url) {
            return Err((about.blocked)());
        }

        match &self.source {
            Source::Live(live) => live.body(url, self.limit, about).await,

            Source::Fixture(fixture) => weighed(fixture.body(url, about)?, self.limit, about),
        }
    }
}

fn weighed(body: Body, limit: u64, about: &About<'_>) -> Result<Body, Failure> {
    let weight = body.len() as u64;
    if weight > limit {
        return Err((about.too_large)(weight));
    }
    Ok(body)
}

pub enum Body {
    Owned(Vec<u8>),
    Shared(Arc<[u8]>),
}

impl std::ops::Deref for Body {
    type Target = [u8];

    fn deref(&self) -> &[u8] {
        match self {
            Self::Owned(bytes) => bytes,
            Self::Shared(bytes) => bytes,
        }
    }
}

impl std::fmt::Debug for Body {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Body({} bytes)", self.len())
    }
}
