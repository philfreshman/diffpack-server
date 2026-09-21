//! What a registry says a package's versions are.
//!
//! One interface — [`Catalogue::versions`] — and everything a package's
//! release history costs on the far side of it: where the document is, the
//! HTTP client, the size cap, and reading three different shapes of JSON into
//! one answer. A tool asks for a package's versions and is given them newest
//! first or a [`Failure`]; which registry spells its dates which way is not a
//! tool's to know.
//!
//! The same shape as [`crate::archive`] and for the same reasons: one seam,
//! two adapters, and [`crate::fetch`] underneath both so there is one HTTP
//! client in this crate rather than one per seam.
//!
//! # Why this is not the archive seam
//!
//! [ADR 0001](../docs/adr/0001-the-archive-seam-is-a-filemap.md) says the
//! archive seam *is* a `FileMap` — bytes downloaded, decompressed, untarred,
//! stripped. A version list is none of those things: it is a document that is
//! read rather than extracted, it is about a package rather than about one
//! version of one, and it is never cached. Putting it behind `Archive::fetch`
//! would have made that ADR's sentence untrue.
//!
//! # Nothing here is cached
//!
//! The 256 MB budget belongs to diff results. What a registry says a package
//! has released goes stale the moment somebody publishes, so it is fetched
//! per call rather than written to the blob store (#18).
//!
//! #18 asked for a short `ttlMs` on the answer as the freshness signal
//! instead. A `tools/call` result has nowhere to put one: in the
//! `2026-07-28` schema `CacheableResult` is extended by `DiscoverResult`,
//! the four list results and `ReadResourceResult`, while `CallToolResult`
//! extends plain `Result`. So the hint belongs to the `diffpack://` resource
//! #16 adds, and until then fetching per call is the whole of the freshness
//! story — every answer is as fresh as the registry was when it was asked.

mod fixture;
mod live;

use std::path::PathBuf;

use crate::error::Failure;
use crate::registry::{Registry, Version};

/// The most a version document may weigh before this server refuses it
/// unread.
///
/// Sized against the documents npm actually serves rather than against a
/// round number: `typescript`'s is 15.7 MB and `@types/node`'s is 10.6 MB,
/// because npm's full metadata carries every version's manifest and the
/// abbreviated form that does not carry the publish dates this seam is here
/// for. A cap near the largest real document would refuse the next package
/// that grows; no cap at all makes a package name in a tool argument into a
/// way to fill this function's memory from somebody else's server.
///
/// Well under [`crate::archive::SIZE_LIMIT`] on purpose: an 80 MB crate is an
/// ordinary thing to diff, and an 80 MB list of version numbers is not an
/// ordinary anything.
pub const SIZE_LIMIT: u64 = 32 * 1024 * 1024;

/// Where a package's versions come from.
#[derive(Debug)]
pub struct Catalogue {
    source: Source,
    /// The most a document may weigh before this server refuses it. A field
    /// rather than a constant read at the point of use, so that the refusal
    /// can be exercised with a real document and a small limit.
    limit: u64,
}

/// The two adapters, as a variant each rather than a trait — the same
/// reasoning as [`crate::archive`] and [ADR
/// 0004](../docs/adr/0004-one-registry-module.md).
#[derive(Debug)]
enum Source {
    Live(live::Live),
    Fixture(fixture::Fixture),
}

impl Catalogue {
    /// Versions from the registries, which is what production runs.
    pub fn live() -> Self {
        Self {
            source: Source::Live(live::Live::new()),
            limit: SIZE_LIMIT,
        }
    }

    /// Version documents read from `dir` rather than from a registry.
    ///
    /// `dir` holds an `index.json` mapping a URL to the file beside it that
    /// stands in for what that URL serves, exactly as
    /// [`crate::archive::Archive::fixture`] does. Keyed by URL on purpose: a
    /// fetch that built a URL of its own rather than asking
    /// [`crate::registry`] would find nothing here.
    pub fn fixture(dir: impl Into<PathBuf>) -> Self {
        Self {
            source: Source::Fixture(fixture::Fixture::new(dir.into())),
            limit: SIZE_LIMIT,
        }
    }

    /// The same source, refusing anything over `limit` bytes.
    pub fn with_limit(self, limit: u64) -> Self {
        Self { limit, ..self }
    }

    /// Every published version of `package`, newest first.
    pub async fn versions(
        &self,
        registry: Registry,
        package: &str,
    ) -> Result<Vec<Version>, Failure> {
        let url = registry.versions(package).url;
        let document = self.bytes(&url, registry, package).await?;

        let document = String::from_utf8(document)
            .map_err(|_| unreadable(registry, package, "what the registry served is not text"))?;

        registry.read_versions(&document).ok_or_else(|| {
            unreadable(
                registry,
                package,
                "the registry answered in a shape this server does not know",
            )
        })
    }

    /// Whatever is served at `url`, as bytes, or a refusal if it is larger
    /// than this server will read.
    async fn bytes(
        &self,
        url: &str,
        registry: Registry,
        package: &str,
    ) -> Result<Vec<u8>, Failure> {
        // Every outbound request, checked against the hosts the registries
        // between them name — the same check [`crate::archive`] makes, and
        // for the same reason: a URL this crate built is allowed by
        // construction, and checking it anyway costs one line and leaves
        // nothing to keep in step.
        if !crate::registry::allows(url) {
            return Err(unreadable(
                registry,
                package,
                "they are served from a host this server does not fetch from",
            ));
        }

        let bytes = match &self.source {
            Source::Live(live) => live.bytes(url, self.limit, registry, package).await?,
            Source::Fixture(fixture) => fixture.bytes(url, registry, package)?,
        };

        let weight = bytes.len() as u64;
        if weight > self.limit {
            return Err(too_large(registry, package, weight, self.limit));
        }
        Ok(bytes)
    }
}

/// A package this server cannot read a version list for, in the words a model
/// reads.
///
/// It is [`Failure::MalformedArchive`]'s counterpart for a document rather
/// than an archive, and it says which half went wrong for the same reason:
/// metadata that is not JSON is the registry serving something broken, and a
/// host outside the allowlist is a redirect nobody should follow. Neither is
/// a retry.
fn unreadable(registry: Registry, package: &str, reason: &str) -> Failure {
    Failure::UnreadableVersions {
        registry: registry.name().to_owned(),
        package: package.to_owned(),
        reason: reason.to_owned(),
    }
}

/// A package the registry does not have, in the words a model reads.
///
/// One constructor for both adapters, so the fixture set cannot answer
/// something the registries would not. A `404` here is about the *package*,
/// which is what makes this seam's refusal different from
/// [`crate::archive`]'s: there is no version in the request to have got wrong.
pub(super) fn no_such_package(registry: Registry, package: &str) -> Failure {
    Failure::NoSuchPackage {
        registry: registry.name().to_owned(),
        package: package.to_owned(),
    }
}

/// A version list this server declined to read, in the words a model reads.
fn too_large(registry: Registry, package: &str, bytes: u64, limit: u64) -> Failure {
    Failure::VersionsTooLarge {
        registry: registry.name().to_owned(),
        package: package.to_owned(),
        bytes,
        limit,
    }
}
