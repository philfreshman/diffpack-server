//! Everything that goes wrong here, and which channel it reaches the client
//! on.
//!
//! An agent can only recover from a failure it can read, and MCP gives it two
//! places to read one. Conflating them is the usual mistake, so this module
//! settles it once rather than leaving nineteen tools to each decide:
//!
//! * A **protocol error** is a JSON-RPC error. The method does not exist, the
//!   parameters do not validate, the URI does not resolve. Clients render
//!   these opaquely if at all — the model does not see the message, so there
//!   is no point writing one for it.
//! * A **tool error** is a *successful* JSON-RPC response carrying
//!   `isError: true`. The model does see it. "crates.io has no version 9.9.9
//!   of serde" belongs here: the recovery is to ask for a version that
//!   exists, and only the model can do that.
//!
//! [`Failure::respond`] is the whole rule. Its return type is exactly a tool
//! handler's — `Result<CallToolResult, ErrorData>` — so a handler that ends
//! in `failure.respond()` cannot put a failure on the wrong channel by
//! accident. `Err` is the protocol; `Ok` with `isError` is the model's.
//!
//! # What does not appear in a message
//!
//! No token, no signed URL, no path inside the function. The defence is
//! mostly structural: a message is built from the fields named in a variant,
//! never from an upstream error's own `Display`, so there is nothing for a
//! credential to arrive in. The two fields that do carry free text from
//! elsewhere go through [`redact`] on the way out.

use std::borrow::Cow;
use std::fmt::Write as _;
use std::future::Future;
use std::time::Duration;

use rmcp::model::{CallToolResult, ContentBlock, ErrorCode, ErrorData};

/// How long any one outbound request may take.
///
/// Chosen against the handler's `maxDuration` in `vercel.json`, not picked for
/// roundness: a request that outlives the function is worse than a slow one,
/// because Vercel kills the process and the client gets a dropped connection
/// instead of a message naming the registry that stalled. The margin has to
/// cover more than one outbound request plus the diff itself, which is what
/// `tests/errors.rs` holds it to.
pub const UPSTREAM_TIMEOUT: Duration = Duration::from_secs(30);

/// The JSON-RPC code for a failure of ours.
///
/// JSON-RPC reserves `-32768..-32000`; within it the MCP specification has
/// taken `-32020..-32099` for itself — rmcp already carries three codes there
/// — leaving `-32000..-32019` to an implementation. One code is enough while
/// there is one kind of failure that is nobody's fault but ours; a second
/// goes beside it rather than outside the range.
const INTERNAL_FAILURE: ErrorCode = ErrorCode(-32000);

/// Everything this server can fail at.
///
/// Registry failures are separate variants rather than one string because
/// they are separate problems: a rate limit means wait, a missing package
/// means check the name, a malformed archive means the registry is serving
/// something broken. Collapsing them would leave a model guessing which
/// remedy it is looking at.
#[derive(Debug)]
#[non_exhaustive]
pub enum Failure {
    /// The registry has no package by that name.
    ///
    /// `registry` is the registry's own name for itself — `npm`,
    /// `crates.io`, `PyPI` — because that is what a model has seen elsewhere
    /// and what a user would search for. #10 owns where those strings come
    /// from.
    NoSuchPackage { registry: String, package: String },

    /// The registry has the package, but not that version.
    ///
    /// `known` is the way forward: without it the message is a `404` with
    /// extra words, and the model has no next call to make.
    NoSuchVersion {
        registry: String,
        package: String,
        version: String,
        known: Vec<String>,
    },

    /// The registry asked us to slow down.
    RateLimited {
        registry: String,
        retry_after: Option<Duration>,
    },

    /// The registry did not answer inside [`UPSTREAM_TIMEOUT`].
    TimedOut { registry: String, waited: Duration },

    /// The registry could not be reached at all.
    ///
    /// A name that did not resolve, a refused connection, a TLS handshake
    /// that failed: there is no status to report, because there was no
    /// answer. Distinct from [`Failure::Unavailable`], which is a registry
    /// that answered and said no — the remedies differ, since that one is
    /// about the registry's health and this one is as likely to be about
    /// ours.
    ///
    /// No cause is carried. What a client would learn from a resolver's
    /// complaint is nothing it can act on, and it is free text from a library
    /// arriving in a message.
    Unreachable { registry: String },

    /// The registry answered, but with nothing usable.
    Unavailable { registry: String, status: u16 },

    /// The archive arrived and would not extract.
    ///
    /// `reason` is free text from whatever failed to read it, so it is
    /// redacted on the way out.
    MalformedArchive {
        package: String,
        version: String,
        reason: String,
    },

    /// The archive is larger than this function will hold.
    TooLarge {
        package: String,
        version: String,
        bytes: u64,
        limit: u64,
    },

    /// The version has no file at that path.
    ///
    /// The commonest way an agent arrives here is by writing the archive's
    /// top-level directory back into the path it was given without one, so
    /// the message says that rather than only saying no.
    NoSuchFile {
        package: String,
        version: String,
        path: String,
    },

    /// The path names a directory, which has no content to return.
    ///
    /// Distinct from a path that is not there at all, and deliberately: the
    /// extractor gives a directory the empty string, so the two would
    /// otherwise both arrive as nothing and a model could not tell "this
    /// module ships no code" from "I spelled the path wrong". The remedies
    /// differ too — one is to ask for a file inside, the other is to find out
    /// what the paths are.
    PathIsDirectory {
        package: String,
        version: String,
        path: String,
    },

    /// One item of an answer is larger than a whole response.
    ///
    /// Distinct from [`Failure::TooLarge`], which is about an archive this
    /// server declined to download. This one is about an answer it built and
    /// cannot send: a single entry whose serialised form is over the
    /// response ceiling fits on no page, however small the `limit`.
    ///
    /// `resume` is the way forward, and it is why this is a refusal rather
    /// than a silent skip. Dropping the item would leave a walk that claims
    /// to have covered the sequence and has not — a wrong answer an agent
    /// cannot detect. Naming the cursor that continues past it makes the skip
    /// the agent's decision instead of ours.
    ItemTooLarge {
        position: usize,
        bytes: usize,
        ceiling: usize,
        resume: String,
    },

    /// A registry whose archive URL cannot be built from a package name and
    /// a version.
    ///
    /// npm and crates.io serve an archive from a path anyone can construct.
    /// PyPI lists a version's files in its own metadata and nowhere else, so
    /// there is nothing to build — which is a property of that registry
    /// rather than a gap here. #10 answers it by asking PyPI.
    ///
    /// `resolvable` is the way forward, and it comes from the caller for the
    /// same reason [`Failure::NoSuchVersion`]'s `known` does: the registries
    /// that can be resolved are [`crate::registry`]'s to know, and a list
    /// written out here would be a copy that #28 has to find.
    UnresolvableArchiveUrl {
        registry: String,
        resolvable: Vec<String>,
    },

    /// The caller's parameters did not validate.
    InvalidParams { message: String },

    /// A tool name that is not one of ours.
    ///
    /// A protocol error for the same reason [`Failure::NoSuchResource`] is:
    /// there is no tool to have failed, so there is no tool error to report,
    /// and the client — which was told the list — is who can fix the call.
    NoSuchTool { name: String },

    /// A resource URI that does not resolve.
    ///
    /// A protocol error, and since `2026-07-28` an *invalid parameters* one:
    /// the code moved from `-32002` to `-32602`, which is what it always
    /// described.
    NoSuchResource { uri: String },

    /// Something inside this server failed.
    ///
    /// `doing` is a fixed phrase chosen at the call site and not a cause
    /// threaded out from below — a cause is what carries a token or a path,
    /// and the client is not who needs it. The cause belongs in the log
    /// (#26), where it can be read next to the request that produced it.
    Internal { doing: &'static str },
}

impl Failure {
    /// Put this failure on the channel it belongs to.
    ///
    /// `Ok` is a tool error the model reads and can act on; `Err` is a
    /// protocol error it never sees. The return type is a tool handler's, so
    /// `failure.respond()` is the whole of a handler's error path.
    pub fn respond(self) -> Result<CallToolResult, ErrorData> {
        match self {
            // The caller's fault, or nobody's: there is nothing a model can
            // do with these, so they do not go where a model would read them.
            Self::InvalidParams { ref message } => Err(ErrorData::invalid_params(
                format!("Invalid parameters: {}", redact(message)),
                None,
            )),
            Self::NoSuchResource { ref uri } => Err(ErrorData::invalid_params(
                format!("No resource at `{}`.", redact(uri)),
                None,
            )),
            Self::NoSuchTool { ref name } => Err(ErrorData::invalid_params(
                format!("No tool named `{}`.", redact(name)),
                None,
            )),
            Self::Internal { doing } => Err(ErrorData::new(
                INTERNAL_FAILURE,
                format!("diffpack failed while {doing}."),
                None,
            )),

            // Everything else happened to a tool that ran, and is something
            // the model can do something about.
            other => Ok(CallToolResult::error(vec![ContentBlock::text(
                other.message(),
            )])),
        }
    }

    /// The text a model reads.
    ///
    /// Written for someone who cannot see our logs: what was asked for, what
    /// happened, and — where there is one — what would work instead. The
    /// transient failures say "again" and the permanent ones do not, so that
    /// a model can tell a retry from a dead end without parsing prose.
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
                    // `write!` to a String cannot fail; the `let _` keeps
                    // clippy from asking about the Result.
                    let _ = write!(message, " Known recent versions: {}.", known.join(", "));
                }
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

            Self::Unreachable { registry } => {
                format!("{registry} could not be reached from this server. Try again shortly.")
            }

            Self::Unavailable { registry, status } => format!(
                "{registry} answered with HTTP {status}, which this server cannot use. \
                 Try again shortly."
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

            // These four never reach a model — `respond` sends them down the
            // protocol channel — but a `message` that lied about them would
            // be a trap for the next person to add a variant.
            Self::InvalidParams { message } => redact(message),
            Self::NoSuchResource { uri } => format!("No resource at `{}`.", redact(uri)),
            Self::Internal { doing } => format!("diffpack failed while {doing}."),
            Self::NoSuchTool { name } => format!("No tool named `{}`.", redact(name)),
        }
    }
}

/// Run `future`, giving it [`UPSTREAM_TIMEOUT`] and no more.
///
/// The budget is here rather than at each call site so that "every outbound
/// request has a timeout" is something the type system helps with: a future
/// that reaches the network goes through this, and one that does not cannot
/// hang the function anyway.
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

/// Remove from `text` anything that must not leave the process.
///
/// This is the second line of defence, not the first: messages are built from
/// fields this crate chose, so there is normally nothing here to catch. It
/// exists for the fields that carry text from somewhere else — an extractor's
/// complaint, a store's refusal — where a path or a signed URL could arrive
/// without anyone deciding it should.
///
/// Three shapes, because three shapes are what this deployment has:
///
/// * a URL's query and fragment, which is where a signature or a token rides;
/// * a Vercel Blob token, which is a single word with a recognisable prefix;
/// * an absolute path inside the function's filesystem.
///
/// Anything it does not recognise it leaves alone. A redactor that ate the
/// message would be its own failure — an error nobody can read is as useless
/// as one that leaks.
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
    // A Blob read-write token. The whole word goes: there is no part of a
    // credential that is safe to show.
    if word.contains("vercel_blob_") || word.contains("_rw_") {
        return Cow::Owned("[redacted credential]".to_owned());
    }

    // A URL. The origin and path say which store refused us, which is worth
    // keeping; the query and fragment are where a signature lives.
    if let Some(scheme_end) = word.find("://") {
        let after_scheme = scheme_end + "://".len();
        if let Some(cut) = word[after_scheme..].find(['?', '#']) {
            return Cow::Owned(format!("{}?[redacted]", &word[..after_scheme + cut]));
        }
        return Cow::Borrowed(word);
    }

    // A path inside the function. `/var/task` is where Vercel unpacks the
    // deployment and `/tmp` is the only writable directory, so between them
    // they are every absolute path this process can produce.
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
