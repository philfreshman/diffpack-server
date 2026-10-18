use std::collections::HashMap;
use std::path::PathBuf;

use crate::document::{About, Document};
use crate::engine;
use crate::error::Failure;
use crate::registry::{ArchiveSource, Registry};

#[derive(Debug)]
pub struct FileMap {
    entries: HashMap<String, engine::FileMapEntry>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum At<'a> {
    File(File<'a>),

    Directory,

    Nothing,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct File<'a> {
    text: &'a str,
}

impl FileMap {
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

    pub fn paths(&self) -> Vec<&str> {
        let mut paths: Vec<&str> = self.entries.keys().map(String::as_str).collect();
        paths.sort_unstable();
        paths
    }

    pub(crate) fn as_engine_map(&self) -> &HashMap<String, engine::FileMapEntry> {
        &self.entries
    }
}

impl From<HashMap<String, engine::FileMapEntry>> for FileMap {
    fn from(entries: HashMap<String, engine::FileMapEntry>) -> Self {
        Self { entries }
    }
}

impl<'a> At<'a> {
    pub fn text(self) -> Option<&'a str> {
        match self {
            At::File(file) => Some(file.text()),
            At::Directory | At::Nothing => None,
        }
    }
}

impl<'a> File<'a> {
    pub fn text(self) -> &'a str {
        self.text
    }

    pub fn decoded_cleanly(self) -> bool {
        decoded_cleanly(self.text)
    }
}

pub fn decoded_cleanly(text: &str) -> bool {
    !text.contains(char::REPLACEMENT_CHARACTER)
}

pub const SIZE_LIMIT: u64 = 128 * 1024 * 1024;

const NO_SUCH_VERSION: u16 = 404;

#[derive(Debug)]
pub struct Archive {
    documents: Document,
}

impl Archive {
    pub fn live() -> Self {
        Self {
            documents: Document::live(SIZE_LIMIT),
        }
    }

    pub fn fixture(dir: impl Into<PathBuf>) -> Self {
        Self {
            documents: Document::fixture(dir, "reading the archive fixtures", SIZE_LIMIT),
        }
    }

    pub fn with_limit(self, limit: u64) -> Self {
        Self {
            documents: self.documents.with_limit(limit),
        }
    }

    pub async fn fetch(
        &self,
        registry: Registry,
        package: &str,
        version: &str,
    ) -> Result<FileMap, Failure> {
        let limit = self.documents.limit();
        let about = About {
            registry,
            accept: None,
            compressed: false,
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
        extract(registry, &bytes, package, version)
    }
}

fn extract(
    registry: Registry,
    bytes: &[u8],
    package: &str,
    version: &str,
) -> Result<FileMap, Failure> {
    engine::unpack_archive(registry.id(), package, version, bytes)
        .map(FileMap::from)
        .map_err(|reason| Failure::MalformedArchive {
            package: package.to_owned(),
            version: version.to_owned(),
            reason,
        })
}

fn unreadable(package: &str, version: &str, reason: &str) -> Failure {
    Failure::MalformedArchive {
        package: package.to_owned(),
        version: version.to_owned(),
        reason: reason.to_owned(),
    }
}

fn not_found(registry: Registry, package: &str, version: &str) -> Failure {
    Failure::NoSuchVersion {
        registry: registry.name().to_owned(),
        package: package.to_owned(),
        version: version.to_owned(),
        known: Vec::new(),
    }
}

fn too_large(package: &str, version: &str, bytes: u64, limit: u64) -> Failure {
    Failure::TooLarge {
        package: package.to_owned(),
        version: version.to_owned(),
        bytes,
        limit,
    }
}
