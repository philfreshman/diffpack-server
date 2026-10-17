use std::path::PathBuf;

use crate::document::{About, Document};
use crate::error::Failure;
use crate::registry::{Registry, Versions};

pub const SIZE_LIMIT: u64 = 32 * 1024 * 1024;

const NO_SUCH_PACKAGE: u16 = 404;

#[derive(Debug)]
pub struct Catalogue {
    documents: Document,
}

impl Catalogue {
    pub fn live() -> Self {
        Self {
            documents: Document::live(SIZE_LIMIT),
        }
    }

    pub fn fixture(dir: impl Into<PathBuf>) -> Self {
        Self {
            documents: Document::fixture(dir, "reading the version fixtures", SIZE_LIMIT),
        }
    }

    pub fn with_limit(self, limit: u64) -> Self {
        Self {
            documents: self.documents.with_limit(limit),
        }
    }

    pub async fn versions(&self, registry: Registry, package: &str) -> Result<Versions, Failure> {
        let limit = self.documents.limit();
        let source = registry.versions(package);

        let document = self
            .documents
            .body(
                &source.url,
                &About {
                    registry,
                    accept: None,
                    compressed: false,
                    remember_for: None,
                    nothing_there: NO_SUCH_PACKAGE,
                    blocked: &|| {
                        unreadable(
                            registry,
                            package,
                            "they are served from a host this server does not fetch from",
                        )
                    },
                    missing: &|_| no_such_package(registry, package),
                    too_large: &|bytes| too_large(registry, package, bytes, limit),
                },
            )
            .await?;

        let document = std::str::from_utf8(&document)
            .map_err(|_| unreadable(registry, package, "what the registry served is not text"))?;

        registry.read_versions(document).ok_or_else(|| {
            unreadable(
                registry,
                package,
                "the registry answered in a shape this server does not know",
            )
        })
    }
}

fn unreadable(registry: Registry, package: &str, reason: &str) -> Failure {
    Failure::UnreadableVersions {
        registry: registry.name().to_owned(),
        package: package.to_owned(),
        reason: reason.to_owned(),
    }
}

fn no_such_package(registry: Registry, package: &str) -> Failure {
    Failure::NoSuchPackage {
        registry: registry.name().to_owned(),
        package: package.to_owned(),
    }
}

fn too_large(registry: Registry, package: &str, bytes: u64, limit: u64) -> Failure {
    Failure::VersionsTooLarge {
        registry: registry.name().to_owned(),
        package: package.to_owned(),
        bytes,
        limit,
    }
}
