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
//!
//! Neither adapter is written here any more. Both are
//! [`crate::document`]'s — the four steps this module shares with
//! [`crate::catalogue`] and [`crate::search`], written once — and what is
//! left in this file is the two things that are only an archive's: which URL
//! a version's bytes are at, including the hop PyPI needs, and what those
//! bytes are once they arrive. The interface did not move and neither did any
//! of the refusals; see [ADR
//! 0015](../docs/adr/0015-one-implementation-beneath-three-registry-seams.md).

use std::collections::HashMap;
use std::path::PathBuf;

use crate::document::{About, Document};
use crate::engine;
use crate::error::Failure;
use crate::registry::{ArchiveSource, Registry};

/// An archive after extraction: every path in a version and what is at it,
/// with the archive's top-level directory already stripped.
///
/// The boundary the rest of the crate sees. A tool asks for one of these and
/// never for the bytes it was made from.
///
/// # Three questions, and nothing else
///
/// What is at a path ([`FileMap::at`]), whether a file decoded cleanly
/// ([`File::decoded_cleanly`]), and the paths in order ([`FileMap::paths`]).
/// The map the extractor built is private, because what one of its entries
/// means is not something a caller can read off it: a directory's content is
/// the empty string, so emptiness cannot tell a directory from an empty file,
/// and a file that was not UTF-8 holds replacement characters rather than
/// its bytes. Five places in the tools used to work that out for themselves,
/// and four of them carried a comment saying how (#95). A change to
/// extraction — a symlink, or a flag for bytes that did not decode — is now an
/// edit here rather than five.
///
/// The one thing that still reads the map is the engine, which builds a tree
/// out of two of them. That goes through [`crate::engine::build_diff_tree`],
/// so the engine's entry type is still named in one file (ADR 0007).
#[derive(Debug)]
pub struct FileMap {
    entries: HashMap<String, engine::FileMapEntry>,
}

/// What is at one path of a [`FileMap`].
///
/// Three answers, and a caller matching on them is the one place a tool
/// decides what each means to it: `get_file_content` refuses the second and
/// the third, `list_package_files` lists the first two, and a patch reads
/// both of the last two as nothing to diff.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum At<'a> {
    /// A file, with its text.
    File(File<'a>),

    /// A directory, which has no text of its own. Not an empty file.
    Directory,

    /// Nothing: the version has no such path.
    Nothing,
}

/// One file of a [`FileMap`], as extraction left it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct File<'a> {
    text: &'a str,
}

impl FileMap {
    /// What is at `path`.
    ///
    /// The type is what answers, never the content. A directory's content is
    /// the empty string the extractor gave it, so reading emptiness would
    /// call every empty file a directory.
    pub fn at(&self, path: &str) -> At<'_> {
        match self.entries.get(path) {
            None => At::Nothing,
            Some(entry) => match entry.file_type {
                engine::FileType::File => At::File(File {
                    text: &entry.content,
                }),
                engine::FileType::Directory => At::Directory,
            },
        }
    }

    /// Every path in the version, files and directories alike, in order.
    ///
    /// The order is the paths' own, byte by byte. The map underneath has none
    /// of its own, and two calls that each walked it would not agree on one,
    /// which is what a cursor into a listing needs them to do.
    pub fn paths(&self) -> Vec<&str> {
        let mut paths: Vec<&str> = self.entries.keys().map(String::as_str).collect();
        paths.sort_unstable();
        paths
    }

    /// The map as the engine reads it, for [`crate::engine`] to hand back.
    ///
    /// Nothing else calls this. A caller that read the entries off it would
    /// be working out again what [`FileMap::at`] answers.
    pub(crate) fn as_engine_map(&self) -> &HashMap<String, engine::FileMapEntry> {
        &self.entries
    }
}

/// A map as the extractor builds it.
///
/// Public so a test can build a FileMap by hand, the way `tests/engine.rs`
/// builds two for the tree builder. Reading one back is still the three
/// questions above.
impl From<HashMap<String, engine::FileMapEntry>> for FileMap {
    fn from(entries: HashMap<String, engine::FileMapEntry>) -> Self {
        Self { entries }
    }
}

impl<'a> At<'a> {
    /// The text of the file at this path, or nothing where there is no file.
    ///
    /// A directory is nothing here, which is how the engine reads one when it
    /// renders a patch: it has no text to diff. A caller that has to tell a
    /// directory from an absent path matches on the answer instead.
    pub fn text(self) -> Option<&'a str> {
        match self {
            At::File(file) => Some(file.text()),
            At::Directory | At::Nothing => None,
        }
    }
}

impl<'a> File<'a> {
    /// The file's text, decoded the way extraction decodes it.
    pub fn text(self) -> &'a str {
        self.text
    }

    /// Whether every byte of the file decoded as UTF-8, so the text is the
    /// file.
    ///
    /// Read off the text rather than off the bytes, because the bytes are
    /// gone: extraction decodes lossily, and what a FileMap holds is the
    /// result. The cost is that a text file that really does contain a
    /// replacement character is reported as not decoding cleanly. That is
    /// rare, and it is the safe direction to be wrong in: an agent told a
    /// file may not have decoded reads it more carefully, where one told a
    /// binary is clean text does not.
    ///
    /// Always the whole file. A caller that cut the text first and asked of
    /// the cut would report a binary as clean wherever the cut fell before
    /// the first byte that did not decode.
    pub fn decoded_cleanly(self) -> bool {
        !self.text.contains(char::REPLACEMENT_CHARACTER)
    }
}

/// The most any one body this server downloads may weigh.
///
/// Sized against the packages people actually diff rather than against the
/// function's memory: an 80 MB crate is an ordinary thing to ask for, and
/// nothing a registry serves legitimately is anywhere near five gigabytes. A
/// cap below the first would refuse everyday work, and no cap at all makes a
/// package name in a tool argument into a way to fill this function's memory
/// from somebody else's server.
pub const SIZE_LIMIT: u64 = 128 * 1024 * 1024;

/// What a `404` stands in for in the archive fixture set: a version the
/// registry does not have.
///
/// Which of the three statuses it was does not change what a model does about
/// a version that is not there, so this is the commonest of them rather than
/// a choice between them.
const NO_SUCH_VERSION: u16 = 404;

/// Where a version's files come from.
#[derive(Debug)]
pub struct Archive {
    documents: Document,
}

impl Archive {
    /// Archives from the registries, which is what production runs.
    pub fn live() -> Self {
        Self {
            documents: Document::live(SIZE_LIMIT),
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
            documents: Document::fixture(dir, "reading the archive fixtures", SIZE_LIMIT),
        }
    }

    /// The same archive source, refusing anything over `limit` bytes.
    pub fn with_limit(self, limit: u64) -> Self {
        Self {
            documents: self.documents.with_limit(limit),
        }
    }

    /// The files in `version` of `package`.
    pub async fn fetch(
        &self,
        registry: Registry,
        package: &str,
        version: &str,
    ) -> Result<FileMap, Failure> {
        // What an archive request is, in the words its failures need. One
        // `About` for both hops: a metadata document is a download too, and
        // one that is somehow enormous or served from somewhere unexpected is
        // the same problem arriving a hop earlier.
        let limit = self.documents.limit();
        let about = About {
            registry,
            // One representation at each of these URLs, so there is nothing
            // to ask for by name.
            accept: None,
            // An archive is compressed already; negotiating it again would
            // trade the cheap half of the cap for nothing.
            compressed: false,
            // Nothing here is worth holding between invocations: an archive
            // is fetched for one comparison, and the thing worth keeping is
            // the comparison, which is `crate::store`'s.
            remember_for: None,
            nothing_there: NO_SUCH_VERSION,
            blocked: &|| {
                unreadable(
                    package,
                    version,
                    "it is served from a host this server does not fetch from",
                )
            },
            missing: &|_| not_found(registry, package, version),
            too_large: &|bytes| too_large(package, version, bytes, limit),
        };

        let url = match registry.archive(package, version)? {
            ArchiveSource::Archive { url } => url,

            // The second hop, and the reason this seam is `fetch` rather than
            // a URL: which of a version's files is the one to diff is the
            // registry's answer to give, and a caller that had to know PyPI
            // needs asking would be carrying this module's job around.
            ArchiveSource::Listing { url } => {
                let listing = self.documents.body(&url, &about).await?;
                let listing = std::str::from_utf8(&listing)
                    .map_err(|_| unreadable(package, version, "its metadata is not text"))?;

                registry.choose_archive(listing).ok_or_else(|| {
                    unreadable(
                        package,
                        version,
                        "the registry lists no archive for it that this server can read",
                    )
                })?
            }
        };

        let bytes = self.documents.body(&url, &about).await?;
        extract(&bytes, package, version)
    }
}

/// An archive's bytes as the files inside it.
///
/// The extractor is [`crate::engine`]'s, which is the code the browser runs:
/// a second implementation here would be two answers to "what is in this
/// version" with nothing keeping them equal.
fn extract(bytes: &[u8], package: &str, version: &str) -> Result<FileMap, Failure> {
    engine::extract_archive_bytes(bytes)
        .map(FileMap::from)
        .map_err(|reason| Failure::MalformedArchive {
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
fn not_found(registry: Registry, package: &str, version: &str) -> Failure {
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
