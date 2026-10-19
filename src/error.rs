use std::borrow::Cow;
use std::fmt::Write as _;
use std::future::Future;
use std::time::Duration;

use rmcp::model::{CallToolResult, ContentBlock, ErrorCode, ErrorData};

pub const UPSTREAM_TIMEOUT: Duration = Duration::from_secs(30);

pub const CALL_DEADLINE: Duration = Duration::from_secs(240);

const INTERNAL_FAILURE: ErrorCode = ErrorCode(-32000);

const ASK_FOR_SOMETHING_ELSE: ErrorCode = ErrorCode(-32001);

const TRY_AGAIN: ErrorCode = ErrorCode(-32003);

const ASK_FOR_LESS: ErrorCode = ErrorCode(-32004);

#[derive(Debug)]
#[non_exhaustive]
pub enum Failure {
    NoSuchPackage {
        registry: String,
        package: String,
    },

    NoSuchVersion {
        registry: String,
        package: String,
        version: String,
        known: Vec<String>,
    },

    RateLimited {
        registry: String,
        retry_after: Option<Duration>,
    },

    TimedOut {
        registry: String,
        waited: Duration,
    },

    Busy {
        waited: Duration,
    },

    OutOfTime {
        deadline: Duration,
    },

    Unreachable {
        registry: String,
    },

    Unavailable {
        registry: String,
        status: u16,
    },

    MalformedArchive {
        package: String,
        version: String,
        reason: String,
    },

    TooLarge {
        package: String,
        version: String,
        bytes: u64,
        limit: u64,
    },

    UnreadableVersions {
        registry: String,
        package: String,
        reason: String,
    },

    VersionsTooLarge {
        registry: String,
        package: String,
        bytes: u64,
        limit: u64,
    },

    UnreadableSearch {
        registry: String,
        reason: String,
    },

    SearchTooLarge {
        registry: String,
        bytes: u64,
        limit: u64,
    },

    NoSuchFile {
        package: String,
        version: String,
        path: String,
    },

    PathIsDirectory {
        package: String,
        version: String,
        path: String,
    },

    ItemTooLarge {
        position: usize,
        bytes: usize,
        ceiling: usize,
        resume: String,
    },

    UnresolvableArchiveUrl {
        registry: String,
        resolvable: Vec<String>,
    },

    InvalidParams {
        message: String,
    },

    NoSuchTool {
        name: String,
    },

    NoSuchResource {
        uri: String,
    },

    NoSuchPrompt {
        name: String,
    },

    Internal {
        doing: &'static str,
    },

    Panicked {
        doing: &'static str,
    },
}

#[derive(Debug, Clone, Copy)]
enum Channel {
    Model(ErrorCode),

    Protocol(ErrorCode),
}

impl Channel {
    const fn code(self) -> ErrorCode {
        match self {
            Self::Model(code) | Self::Protocol(code) => code,
        }
    }
}

impl Failure {
    pub fn respond(self) -> Result<CallToolResult, ErrorData> {
        match self.channel() {
            Channel::Model(_) => Ok(CallToolResult::error(vec![ContentBlock::text(
                self.message(),
            )])),

            Channel::Protocol(code) => Err(ErrorData::new(code, self.message(), None)),
        }
    }

    pub fn refuse(self) -> ErrorData {
        ErrorData::new(self.channel().code(), redact(&self.message()), None)
    }

    fn channel(&self) -> Channel {
        match self {
            Self::NoSuchPackage { .. }
            | Self::NoSuchVersion { .. }
            | Self::MalformedArchive { .. }
            | Self::UnreadableVersions { .. }
            | Self::VersionsTooLarge { .. }
            | Self::UnreadableSearch { .. }
            | Self::NoSuchFile { .. }
            | Self::PathIsDirectory { .. }
            | Self::UnresolvableArchiveUrl { .. } => Channel::Model(ASK_FOR_SOMETHING_ELSE),

            Self::RateLimited { .. }
            | Self::TimedOut { .. }
            | Self::Busy { .. }
            | Self::OutOfTime { .. }
            | Self::Unreachable { .. }
            | Self::Unavailable { .. } => Channel::Model(TRY_AGAIN),

            Self::TooLarge { .. } | Self::SearchTooLarge { .. } | Self::ItemTooLarge { .. } => {
                Channel::Model(ASK_FOR_LESS)
            }

            Self::InvalidParams { .. }
            | Self::NoSuchTool { .. }
            | Self::NoSuchResource { .. }
            | Self::NoSuchPrompt { .. } => Channel::Protocol(ErrorCode::INVALID_PARAMS),

            Self::Internal { .. } => Channel::Protocol(INTERNAL_FAILURE),

            Self::Panicked { .. } => Channel::Protocol(ErrorCode::INTERNAL_ERROR),
        }
    }

    pub fn kind(&self) -> &'static str {
        match self {
            Self::NoSuchPackage { .. } => "no_such_package",
            Self::NoSuchVersion { .. } => "no_such_version",
            Self::RateLimited { .. } => "rate_limited",
            Self::TimedOut { .. } => "timed_out",
            Self::Busy { .. } => "busy",
            Self::OutOfTime { .. } => "out_of_time",
            Self::Unreachable { .. } => "unreachable",
            Self::Unavailable { .. } => "unavailable",
            Self::MalformedArchive { .. } => "malformed_archive",
            Self::TooLarge { .. } => "too_large",
            Self::UnreadableVersions { .. } => "unreadable_versions",
            Self::VersionsTooLarge { .. } => "versions_too_large",
            Self::UnreadableSearch { .. } => "unreadable_search",
            Self::SearchTooLarge { .. } => "search_too_large",
            Self::NoSuchFile { .. } => "no_such_file",
            Self::PathIsDirectory { .. } => "path_is_directory",
            Self::ItemTooLarge { .. } => "item_too_large",
            Self::UnresolvableArchiveUrl { .. } => "unresolvable_archive_url",
            Self::InvalidParams { .. } => "invalid_params",
            Self::NoSuchTool { .. } => "no_such_tool",
            Self::NoSuchResource { .. } => "no_such_resource",
            Self::NoSuchPrompt { .. } => "no_such_prompt",
            Self::Internal { .. } => "internal",
            Self::Panicked { .. } => "panicked",
        }
    }

    fn message(&self) -> String {
        match self {
            Self::NoSuchPackage { registry, package } => {
                format!("{registry} has no package `{package}`. Check the name, or the registry.")
            }

            Self::NoSuchVersion {
                registry,
                package,
                version,
                known,
            } => {
                let mut message =
                    format!("Package `{package}` has no version `{version}` on {registry}.");
                if !known.is_empty() {
                    let _ = write!(message, " Known recent versions: {}.", known.join(", "));
                }
                let _ = write!(
                    message,
                    " Call `list_package_versions` to see every version {registry} has, \
                     spelled the way it spells them."
                );
                message
            }

            Self::RateLimited {
                registry,
                retry_after,
            } => match retry_after {
                Some(wait) => format!(
                    "{registry} is rate limiting this server. Try again in {} seconds.",
                    wait.as_secs()
                ),
                None => format!("{registry} is rate limiting this server. Try again shortly."),
            },

            Self::TimedOut { registry, waited } => format!(
                "{registry} did not answer within {} seconds. Try again.",
                waited.as_secs()
            ),

            Self::Busy { waited } => format!(
                "This server is already fetching as much from registries as it will at \
                 once, and this call's turn did not come within {} seconds. Try again shortly.",
                waited.as_secs()
            ),

            Self::OutOfTime { deadline } => format!(
                "Working this out took longer than the {} seconds this server gives one \
                 call, so this answer was sent instead of waiting longer. Try again once: \
                 a slow registry or a busy server passes. If it runs out of time again, \
                 what was asked for is too large to work out here.",
                deadline.as_secs()
            ),

            Self::Unreachable { registry } => {
                format!("{registry} could not be reached from this server. Try again shortly.")
            }

            Self::Unavailable { registry, status } => format!(
                "{registry} answered with HTTP {status}, which this server cannot use. \
                 Try again shortly."
            ),

            Self::UnreadableVersions {
                registry,
                package,
                reason,
            } => format!(
                "The versions of `{package}` on {registry} could not be read: {}. \
                 There is nothing to retry — ask for a version you already know, \
                 or try another registry.",
                redact(reason),
            ),

            Self::VersionsTooLarge {
                registry,
                package,
                bytes,
                limit,
            } => format!(
                "The versions {registry} has published for `{package}` come to {} MB, over \
                 this server's {} MB limit for them. There is no shorter answer to ask for.",
                bytes / 1_000_000,
                limit / 1_000_000,
            ),

            Self::UnreadableSearch { registry, reason } => format!(
                "{registry} answered that search with something this server could not \
                 read: {}. Try another registry, or ask for a package by the name you \
                 already have.",
                redact(reason),
            ),

            Self::SearchTooLarge {
                registry,
                bytes,
                limit,
            } => format!(
                "{registry}'s answer to that search came to {} MB, over this server's \
                 {} MB limit for one. Try a narrower query, or another registry.",
                bytes / 1_000_000,
                limit / 1_000_000,
            ),

            Self::MalformedArchive {
                package,
                version,
                reason,
            } => format!(
                "The archive for `{package}` {version} could not be read: {}. \
                 The registry is serving a broken or truncated file, so there is nothing \
                 to retry.",
                redact(reason),
            ),

            Self::TooLarge {
                package,
                version,
                bytes,
                limit,
            } => format!(
                "`{package}` {version} is {} MB, over this server's {} MB limit. \
                 Diff a smaller package, or ask for a single file instead of the whole tree.",
                bytes / 1_000_000,
                limit / 1_000_000,
            ),

            Self::NoSuchFile {
                package,
                version,
                path,
            } => format!(
                "`{package}` {version} has no file at `{path}`. List the version's files \
                 to see which paths it has: they have the archive's top-level directory \
                 removed, so a path never begins with the package's own folder."
            ),

            Self::PathIsDirectory {
                package,
                version,
                path,
            } => format!(
                "`{path}` in `{package}` {version} is a directory, not a file, so it has \
                 no content to read. Ask for a file inside it, or list the version's files \
                 to see what it holds."
            ),

            Self::ItemTooLarge {
                position,
                bytes,
                ceiling,
                resume,
            } => format!(
                "Entry {position} of this answer is {bytes} bytes on its own, over the \
                 {ceiling} a single response can carry, so it fits on no page. Ask for that \
                 one entry with a tool that returns it by itself and truncates it, or pass \
                 the cursor `{resume}` to continue past it."
            ),

            Self::UnresolvableArchiveUrl {
                registry,
                resolvable,
            } => {
                let mut message = format!(
                    "`{registry}` does not serve a version's archive from a path that can be \
                     built out of a package name and a version, so there is no URL to resolve."
                );
                if !resolvable.is_empty() {
                    let _ = write!(message, " These do: {}.", resolvable.join(", "));
                }
                message
            }

            Self::InvalidParams { message } => format!("Invalid parameters: {}", redact(message)),
            Self::NoSuchResource { uri } => format!("No resource at `{}`.", redact(uri)),
            Self::Internal { doing } | Self::Panicked { doing } => {
                format!("diffpack failed while {doing}.")
            }
            Self::NoSuchTool { name } => format!("No tool named `{}`.", redact(name)),
            Self::NoSuchPrompt { name } => format!("No prompt named `{}`.", redact(name)),
        }
    }
}

pub async fn within_budget<F>(registry: &str, future: F) -> Result<F::Output, Failure>
where
    F: Future,
{
    tokio::time::timeout(UPSTREAM_TIMEOUT, future)
        .await
        .map_err(|_| Failure::TimedOut {
            registry: registry.to_owned(),
            waited: UPSTREAM_TIMEOUT,
        })
}

pub fn redact(text: &str) -> String {
    text.split_inclusive(char::is_whitespace)
        .map(|word| {
            let (body, trailing) = split_trailing_whitespace(word);
            match redact_word(body) {
                Cow::Borrowed(kept) => format!("{kept}{trailing}"),
                Cow::Owned(replaced) => format!("{replaced}{trailing}"),
            }
        })
        .collect()
}

fn split_trailing_whitespace(word: &str) -> (&str, &str) {
    let end = word.trim_end().len();
    word.split_at(end)
}

fn redact_word(word: &str) -> Cow<'_, str> {
    if word.contains("vercel_blob_") || word.contains("_rw_") {
        return Cow::Owned("[redacted credential]".to_owned());
    }

    if let Some(scheme_end) = word.find("://") {
        let after_scheme = scheme_end + "://".len();
        if let Some(cut) = word[after_scheme..].find(['?', '#']) {
            return Cow::Owned(format!("{}?[redacted]", &word[..after_scheme + cut]));
        }
        return Cow::Borrowed(word);
    }

    if word
        .trim_matches(|c: char| !c.is_ascii_graphic())
        .starts_with("/var/")
        || word.starts_with("/tmp/")
        || word.starts_with("/vercel/")
    {
        return Cow::Owned("[redacted path]".to_owned());
    }

    Cow::Borrowed(word)
}
