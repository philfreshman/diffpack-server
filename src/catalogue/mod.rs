//! What a registry says a package's versions are.
//!
//! One interface — [`Catalogue::versions`] — and everything a package's
//! release history costs on the far side of it: where the document is, the
//! HTTP client, the size cap, and reading three different shapes of JSON into
//! one answer. A tool asks for a package's versions and is given them newest
//! first — with the one the registry itself points at — or a [`Failure`];
//! which registry spells its dates which way, and which field it names its
//! current release in, is not a tool's to know.
//!
//! The same shape as [`crate::archive`] and for the same reasons: one seam,
//! two adapters, and one module under all three seams so there is one HTTP
//! client, one host check and one fixture reader in this crate rather than
//! one of each per seam. That module is [`crate::document`]; what is left
//! here is the part that is only a version list's, which is where the
//! document is and how to read it.
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
//! Sharing what sits *under* the two interfaces does not. See [ADR
//! 0015](../docs/adr/0015-one-implementation-beneath-three-registry-seams.md).
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

use std::path::PathBuf;

use crate::document::{About, Document};
use crate::error::Failure;
use crate::registry::{Registry, Versions};

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

/// What a `404` stands in for in the version fixture set: a package the
/// registry does not have.
///
/// A `404` here is about the *package*, which is what makes this seam's
/// refusal different from [`crate::archive`]'s: there is no version in the
/// request to have got wrong.
const NO_SUCH_PACKAGE: u16 = 404;

/// Where a package's versions come from.
#[derive(Debug)]
pub struct Catalogue {
    documents: Document,
}

impl Catalogue {
    /// Versions from the registries, which is what production runs.
    pub fn live() -> Self {
        Self {
            documents: Document::live(SIZE_LIMIT),
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
            documents: Document::fixture(dir, "reading the version fixtures", SIZE_LIMIT),
        }
    }

    /// The same source, refusing anything over `limit` bytes.
    pub fn with_limit(self, limit: u64) -> Self {
        Self {
            documents: self.documents.with_limit(limit),
        }
    }

    /// Every published version of `package`, newest first, and the one the
    /// registry points at.
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
                    // One document per package, so there is nothing here that
                    // one call's answer would serve another's.
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
/// something the registries would not.
fn no_such_package(registry: Registry, package: &str) -> Failure {
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
