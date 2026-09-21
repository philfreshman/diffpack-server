//! A version's files.
//!
//! One interface — [`Archive::fetch`] — and everything a version's files cost
//! on the far side of it: resolving where the archive is, the metadata hop
//! some registries need, the HTTP client, the size cap, decompressing,
//! untarring and stripping the archive's top-level directory. A tool asks for
//! a [`FileMap`] and is given one or a [`Failure`]; how many requests that
//! took is not a tool's to know. See [ADR
//! 0001](../docs/adr/0001-the-archive-seam-is-a-filemap.md).
//!
//! # Two adapters
//!
//! [`Archive::live`] fetches from the registries. [`Archive::fixture`] reads
//! archives from a directory on disk, which is what lets the suite assert
//! what a version's files are without a network and what lets #24's
//! conformance run in CI. They differ in where the bytes come from and in
//! nothing else: resolution, the size cap, the host allowlist and extraction
//! are the same code for both, so a test through the fixture adapter is a
//! test of the path production takes.

mod fixture;
mod live;

use std::collections::HashMap;
use std::path::PathBuf;

use crate::engine;
use crate::error::Failure;
use crate::registry::{self, ArchiveSource, Registry};

/// An archive after extraction: every file path in a version mapped to its
/// entry, with the archive's top-level directory already stripped.
///
/// The boundary the rest of the crate sees. A tool asks for one of these and
/// never for the bytes it was made from.
pub type FileMap = HashMap<String, engine::FileMapEntry>;

/// The most any one body this server downloads may weigh.
///
/// Sized against the packages people actually diff rather than against the
/// function's memory: an 80 MB crate is an ordinary thing to ask for, and
/// nothing a registry serves legitimately is anywhere near five gigabytes. A
/// cap below the first would refuse everyday work, and no cap at all makes a
/// package name in a tool argument into a way to fill this function's memory
/// from somebody else's server.
pub const SIZE_LIMIT: u64 = 128 * 1024 * 1024;

/// Where a version's files come from.
#[derive(Debug)]
pub struct Archive {
    source: Source,
    /// The most a body may weigh before this server refuses it. A field
    /// rather than a constant read at the point of use, so that the refusal
    /// can be exercised with a real archive and a small limit instead of with
    /// a package nobody wants to download in a test.
    limit: u64,
}

/// The two adapters, as a variant each rather than a trait.
///
/// Neither can arrive from outside this crate, so the extensibility a trait
/// buys has no buyer — and two arms of one `match` are read in a screen
/// where two types are read in two files. The same reasoning as [ADR
/// 0004](../docs/adr/0004-one-registry-module.md).
#[derive(Debug)]
enum Source {
    Live(live::Live),
    Fixture(fixture::Fixture),
}

impl Archive {
    /// Archives from the registries, which is what production runs.
    pub fn live() -> Self {
        Self {
            source: Source::Live(live::Live::new()),
            limit: SIZE_LIMIT,
        }
    }

    /// An archive read from `dir` rather than from a registry.
    ///
    /// `dir` holds an `index.json` mapping a URL to the file beside it that
    /// stands in for what that URL serves. Keyed by URL on purpose: a fetch
    /// path that built a URL of its own rather than asking
    /// [`crate::registry`] would find nothing here.
    pub fn fixture(dir: impl Into<PathBuf>) -> Self {
        Self {
            source: Source::Fixture(fixture::Fixture::new(dir.into())),
            limit: SIZE_LIMIT,
        }
    }

    /// The same archive source, refusing anything over `limit` bytes.
    pub fn with_limit(self, limit: u64) -> Self {
        Self { limit, ..self }
    }

    /// The files in `version` of `package`.
    pub async fn fetch(
        &self,
        registry: Registry,
        package: &str,
        version: &str,
    ) -> Result<FileMap, Failure> {
        let url = match registry.archive(package, version)? {
            ArchiveSource::Archive { url } => url,

            // The second hop, and the reason this seam is `fetch` rather than
            // a URL: which of a version's files is the one to diff is the
            // registry's answer to give, and a caller that had to know PyPI
            // needs asking would be carrying this module's job around.
            ArchiveSource::Listing { url } => {
                let listing = self.bytes(&url, registry, package, version).await?;
                let listing = String::from_utf8(listing)
                    .map_err(|_| unreadable(package, version, "its metadata is not text"))?;

                registry.choose_archive(&listing).ok_or_else(|| {
                    unreadable(
                        package,
                        version,
                        "the registry lists no archive for it that this server can read",
                    )
                })?
            }
        };

        let bytes = self.bytes(&url, registry, package, version).await?;
        extract(&bytes, package, version)
    }

    /// Whatever is served at `url`, as bytes, or a refusal if it is larger
    /// than this server will hold.
    ///
    /// The cap covers every body and not only the archive: a metadata
    /// document is a download too, and one that is somehow enormous is the
    /// same problem arriving a hop earlier.
    async fn bytes(
        &self,
        url: &str,
        registry: Registry,
        package: &str,
        version: &str,
    ) -> Result<Vec<u8>, Failure> {
        // Every outbound request, checked against the hosts the registries
        // between them name — not only the one that could plausibly be
        // wrong. The URL of a first hop is built by `registry` and is
        // allowed by construction; the URL of a second comes out of a
        // document somebody else serves, and a registry that named another
        // host would otherwise have turned a package name in a tool argument
        // into a request wherever it liked. Checking both is one line and
        // leaves nothing to keep in step.
        if !registry::allows(url) {
            return Err(unreadable(
                package,
                version,
                "it is served from a host this server does not fetch from",
            ));
        }

        let bytes = match &self.source {
            Source::Live(live) => {
                live.bytes(url, self.limit, registry, package, version)
                    .await?
            }
            Source::Fixture(fixture) => fixture.bytes(url, registry, package, version)?,
        };

        let weight = bytes.len() as u64;
        if weight > self.limit {
            return Err(too_large(package, version, weight, self.limit));
        }
        Ok(bytes)
    }
}

/// An archive's bytes as the files inside it.
///
/// The extractor is [`crate::engine`]'s, which is the code the browser runs:
/// a second implementation here would be two answers to "what is in this
/// version" with nothing keeping them equal.
fn extract(bytes: &[u8], package: &str, version: &str) -> Result<FileMap, Failure> {
    engine::extract_archive_bytes(bytes).map_err(|reason| Failure::MalformedArchive {
        package: package.to_owned(),
        version: version.to_owned(),
        reason,
    })
}

/// A version this server cannot get an archive for, in the words a model
/// reads.
///
/// `reason` says which half of the two-hop failed, because the two have
/// different remedies and neither is a retry: metadata that is not JSON is
/// the registry serving something broken, and a version with no readable
/// artefact is a package a model should diff at another version.
fn unreadable(package: &str, version: &str, reason: &str) -> Failure {
    Failure::MalformedArchive {
        package: package.to_owned(),
        version: version.to_owned(),
        reason: reason.to_owned(),
    }
}

/// A package or a version the registry does not have, in the words a model
/// reads.
///
/// Which half was wrong is not in a `404` and is not guessed at here: a
/// version is the far commoner mistake, and a model sent to check the name
/// when it mistyped the number has been sent the wrong way. #18 is what will
/// let this carry the versions that do exist.
///
/// One constructor for both adapters, so the fixture set cannot answer
/// something the registries would not.
pub(super) fn not_found(registry: Registry, package: &str, version: &str) -> Failure {
    Failure::NoSuchVersion {
        registry: registry.name().to_owned(),
        package: package.to_owned(),
        version: version.to_owned(),
        known: Vec::new(),
    }
}

/// An archive this server declined to hold, in the words a model reads.
///
/// One constructor for both adapters and both halves of the live one's check,
/// so the refusal says the same thing however it was reached.
fn too_large(package: &str, version: &str, bytes: u64, limit: u64) -> Failure {
    Failure::TooLarge {
        package: package.to_owned(),
        version: version.to_owned(),
        bytes,
        limit,
    }
}
