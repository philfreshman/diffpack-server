use std::path::PathBuf;

use crate::document::{About, Body, Document};
use crate::error::Failure;
use crate::registry::{Registry, Version, Versions};

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
        let source = registry.versions(package);

        let (listed, current) = futures::try_join!(
            self.document(registry, package, &source.url),
            self.current(registry, package, source.current.as_deref()),
        )?;

        let versions = registry
            .read_versions(text(&listed, registry, package)?)
            .ok_or_else(|| unknown_shape(registry, package))?;

        Ok(match current {
            Some(current) => versions.pointing_at(current),
            None => versions,
        })
    }

    async fn current(
        &self,
        registry: Registry,
        package: &str,
        url: Option<&str>,
    ) -> Result<Option<Version>, Failure> {
        let Some(url) = url else {
            return Ok(None);
        };

        let document = self.document(registry, package, url).await?;
        registry
            .read_current(text(&document, registry, package)?)
            .map(Some)
            .ok_or_else(|| unknown_shape(registry, package))
    }

    async fn document(
        &self,
        registry: Registry,
        package: &str,
        url: &str,
    ) -> Result<Body, Failure> {
        let limit = self.documents.limit();

        self.documents
            .body(
                url,
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
            .await
    }
}

fn text<'a>(document: &'a Body, registry: Registry, package: &str) -> Result<&'a str, Failure> {
    std::str::from_utf8(document)
        .map_err(|_| unreadable(registry, package, "what the registry served is not text"))
}

fn unknown_shape(registry: Registry, package: &str) -> Failure {
    unreadable(
        registry,
        package,
        "the registry answered in a shape this server does not know",
    )
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
