use std::path::PathBuf;
use std::time::Duration;

use crate::document::{About, Document};
use crate::error::Failure;
use crate::registry::{Hit, Registry};

pub const SIZE_LIMIT: u64 = 64 * 1024 * 1024;

const FRESH_FOR: Duration = Duration::from_secs(600);

const SOURCE_DOWN: u16 = 503;

const NO_SUCH_PACKAGE: u16 = 404;

#[derive(Debug)]
pub struct Search {
    documents: Document,
}

impl Search {
    pub fn live() -> Self {
        Self {
            documents: Document::live(SIZE_LIMIT),
        }
    }

    pub fn fixture(dir: impl Into<PathBuf>) -> Self {
        Self {
            documents: Document::fixture(dir, "reading the search fixtures", SIZE_LIMIT),
        }
    }

    pub fn with_limit(self, limit: u64) -> Self {
        Self {
            documents: self.documents.with_limit(limit),
        }
    }

    pub async fn hits(
        &self,
        registry: Registry,
        query: &str,
        limit: u32,
    ) -> Result<Vec<Hit>, Failure> {
        let cap = self.documents.limit();
        let source = registry.search(query, limit);

        let answer = self
            .documents
            .body(
                &source.url,
                &About {
                    registry,
                    accept: Some(source.accept),
                    compressed: source.whole_index,
                    remember_for: source.whole_index.then_some(FRESH_FOR),
                    nothing_there: if source.names_a_package {
                        NO_SUCH_PACKAGE
                    } else {
                        SOURCE_DOWN
                    },
                    blocked: &|| Failure::Internal {
                        doing: "asking a registry which packages it has",
                    },
                    missing: &|status| {
                        if source.names_a_package {
                            no_such_package(registry, query)
                        } else {
                            unavailable(registry, status)
                        }
                    },
                    too_large: &|bytes| too_large(registry, bytes, cap),
                },
            )
            .await;

        let body = match answer {
            Err(Failure::NoSuchPackage { .. }) => return Ok(Vec::new()),
            answer => answer?,
        };

        let body = std::str::from_utf8(&body)
            .map_err(|_| unreadable(registry, "what the registry served is not text"))?;

        registry.read_hits(body, query, limit).ok_or_else(|| {
            unreadable(
                registry,
                "the registry answered in a shape this server does not know",
            )
        })
    }
}

fn unavailable(registry: Registry, status: u16) -> Failure {
    Failure::Unavailable {
        registry: registry.name().to_owned(),
        status,
    }
}

fn no_such_package(registry: Registry, query: &str) -> Failure {
    Failure::NoSuchPackage {
        registry: registry.name().to_owned(),
        package: query.to_owned(),
    }
}

fn unreadable(registry: Registry, reason: &str) -> Failure {
    Failure::UnreadableSearch {
        registry: registry.name().to_owned(),
        reason: reason.to_owned(),
    }
}

fn too_large(registry: Registry, bytes: u64, limit: u64) -> Failure {
    Failure::SearchTooLarge {
        registry: registry.name().to_owned(),
        bytes,
        limit,
    }
}
