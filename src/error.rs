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
//! [`Failure::respond`] is the whole rule, and nothing in `src/` calls it but
//! [`crate::tools::call`], once every dispatch has finished. `Err` is
//! the protocol; `Ok` with `isError` is the model's. A handler never reaches
//! it — everything that can go wrong inside one is a [`Failure`] returned
//! upwards — which is what lets the same value be named in a log line before
//! it becomes an answer.
//!
//! # Where there is only one place to read it
//!
//! A `resources/read` has no second channel. `ReadResourceResult` carries
//! contents and nothing else, so there is no `isError` half and every failure
//! is a JSON-RPC error — which means every failure needs a code worth reading,
//! not only the four that were never going to be a tool error.
//!
//! [`Failure::channel`] is where that is decided, and it is decided per
//! variant. It used to be decided by the call site: a read re-coded anything
//! that would have been a tool error as `-32602`, so "this version does not
//! exist" and "this URI is not ours" arrived as one answer and a client had
//! only the prose to separate them. The codes now say what the sentence says
//! — ask for something else, try again, ask for less — for the reader that
//! never gets the sentence. See [ADR
//! 0017](../docs/adr/0017-a-failure-carries-the-code-it-earned.md).
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

/// The JSON-RPC code for a failure that is nobody's fault but ours.
///
/// JSON-RPC reserves `-32768..-32000`; within it the MCP specification has
/// taken `-32020..-32099` for itself — rmcp already carries three codes there
/// — leaving `-32000..-32019` to an implementation. The three below sit beside
/// this one rather than outside the range, for the reason this comment already
/// gave when there was one.
///
/// `-32002` is the hole in the run, and deliberately: it is
/// [`ErrorCode::RESOURCE_NOT_FOUND`], which `2026-07-28` replaced with
/// `-32602` but which rmcp still sends to a peer on an older revision. A code
/// of ours in that slot would reach half the clients in the world as *no such
/// resource*, which is the one sentence these four exist to stop being said.
const INTERNAL_FAILURE: ErrorCode = ErrorCode(-32000);

/// The request was understood, and there is no answer to it.
///
/// The registry has no such package or version, the archive will not extract,
/// the path names a directory, a release history has no shorter form to ask
/// for. Nothing here is transient and nothing here is a malformed request, so
/// neither `-32602` nor a retry is the right thing to tell a client: what is
/// left is to ask for something else. See [`Failure::message`], which says the
/// same thing in a sentence, for the reader that gets one.
const ASK_FOR_SOMETHING_ELSE: ErrorCode = ErrorCode(-32001);

/// Nothing was served, and another attempt might be.
///
/// A rate limit, a timeout, a registry that could not be reached or answered
/// with something unusable, this server's own download queue. The request was
/// fine and so is the thing it asked about; what failed was the attempt.
const TRY_AGAIN: ErrorCode = ErrorCode(-32003);

/// The answer exists, is larger than this server will serve, and has a
/// narrower form.
///
/// Distinct from [`ASK_FOR_SOMETHING_ELSE`] because the remedy is different
/// and a client can act on the difference: the thing asked about is there, and
/// a narrower request for the same thing is the way to it. Distinct from
/// [`TRY_AGAIN`] because asking again unchanged will fail identically.
///
/// All three clauses have to hold, which is why being over a limit is not on
/// its own enough to earn this code. A client reads it as *narrow and ask
/// again*, so a failure with nothing narrower behind it would send that client
/// round the same call for as long as it kept obeying.
const ASK_FOR_LESS: ErrorCode = ErrorCode(-32004);

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

    /// No download slot came free inside the time one call may wait for one.
    ///
    /// This instance already reading as many bodies as it will hold, with a
    /// queue in front of this call deeper than waiting it out is worth. No
    /// registry was asked anything, which is why this is not
    /// [`Failure::TimedOut`]: that message names a registry, and naming one
    /// that was never contacted sends a model to somebody else's status page
    /// for a problem that is ours and transient.
    ///
    /// The only failure here that is about this server's own capacity. Its
    /// remedy is a retry, like a rate limit's — by the time a model asks
    /// again the queue has drained or this instance is not the one serving
    /// it.
    Busy { waited: Duration },

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

    /// The registry's list of a package's versions arrived and could not be
    /// read.
    ///
    /// [`Failure::MalformedArchive`]'s counterpart for a document rather than
    /// an archive, and separate because the remedies are not the same: an
    /// archive that will not extract is one version, and a version list that
    /// will not read takes the whole package with it.
    ///
    /// `reason` is chosen at the call site rather than threaded out from a
    /// parser, so there is no library's free text in it. It is redacted on
    /// the way out regardless.
    UnreadableVersions {
        registry: String,
        package: String,
        reason: String,
    },

    /// The registry's list of a package's versions is larger than this
    /// function will read.
    ///
    /// Distinct from [`Failure::TooLarge`], which is an archive a caller can
    /// do something about by asking for one file instead of a whole tree.
    /// There is no smaller version of a package's release history to ask
    /// for, so the message does not pretend there is.
    VersionsTooLarge {
        registry: String,
        package: String,
        bytes: u64,
        limit: u64,
    },

    /// The registry's answer to a search arrived and could not be read.
    ///
    /// [`Failure::UnreadableVersions`]'s counterpart for a search, and
    /// separate because there is no package in it to name: nothing in the
    /// request was about a package, so the only thing a model can act on is
    /// which registry answered this way.
    ///
    /// `reason` is chosen at the call site rather than threaded out from a
    /// parser, so there is no library's free text in it. It is redacted on
    /// the way out regardless.
    UnreadableSearch { registry: String, reason: String },

    /// The registry's answer to a search is larger than this function will
    /// read.
    ///
    /// Distinct from [`Failure::Unavailable`], which is a registry that
    /// answered and said no: this one answered and kept answering. The
    /// status it did that under is `200`, which is why the refusal does not
    /// carry one — a number that said `200` beside "this server cannot use
    /// it" would read as a contradiction rather than as a fact.
    SearchTooLarge {
        registry: String,
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

/// Which of MCP's channels a [`Failure`] takes, and the code it carries when
/// it travels as a JSON-RPC error.
///
/// Two facts, answered in one match so that they cannot come apart: whether a
/// model can act on this failure, and which code a client reads when there is
/// no model channel to put it on. The code is not conditional on the channel
/// — every failure has one, because every failure can reach a `resources/read`
/// — which is the whole of what #85 changed. It used to be the call site's,
/// and a call site cannot know what went wrong.
#[derive(Debug, Clone, Copy)]
enum Channel {
    /// The model's: a successful response carrying `isError: true`.
    ///
    /// A `tools/call` has that half. A `resources/read` does not, so a failure
    /// that would take this channel and arrives at a read travels as the code
    /// beside it instead, with its message intact.
    Model(ErrorCode),

    /// The client's: a JSON-RPC error, on every surface there is.
    ///
    /// The model does not see one, so the code is most of what is left. These
    /// are the failures a model could do nothing with anyway — a URI that is
    /// not ours, arguments that did not validate, a fault of this server's.
    Protocol(ErrorCode),
}

impl Channel {
    /// The JSON-RPC code, whichever channel this is.
    const fn code(self) -> ErrorCode {
        match self {
            Self::Model(code) | Self::Protocol(code) => code,
        }
    }
}

impl Failure {
    /// Put this failure on the channel it belongs to.
    ///
    /// `Ok` is a tool error the model reads and can act on; `Err` is a
    /// protocol error it never sees. Reached from one place in `src/` —
    /// [`crate::tools::call`], which is the only one that has both the
    /// failure and the answer it becomes — and directly from the suite,
    /// where the channel a failure takes is the thing under test.
    pub fn respond(self) -> Result<CallToolResult, ErrorData> {
        match self.channel() {
            // Something happened to a tool that ran, and it is something the
            // model can do something about.
            Channel::Model(_) => Ok(CallToolResult::error(vec![ContentBlock::text(
                self.message(),
            )])),

            // The caller's fault, or nobody's: there is nothing a model can
            // do with these, so they do not go where a model would read them.
            Channel::Protocol(code) => Err(ErrorData::new(code, self.message(), None)),
        }
    }

    /// The same failure, on the one channel a resource read has.
    ///
    /// A read has no `isError` half to put anything in: `ReadResourceResult`
    /// carries contents and nothing else, so a failure either is a JSON-RPC
    /// error or is not reported. That is why this exists beside
    /// [`Self::respond`] rather than being folded into it — the two channels
    /// are a real choice for a tool call and not a choice at all here, and a
    /// read that reused `respond` would have an `Ok(CallToolResult)` arm with
    /// nowhere to send it.
    ///
    /// What a model loses by that is the message, which is why the message is
    /// carried anyway for the failures that have one. A read of a comparison
    /// can fail the way the tool that computes it fails — a version the
    /// registry does not have — and "no resource at this URI" would be a
    /// worse answer to that than the sentence naming the version. That
    /// argument outlived the function that used to make it: what changed in
    /// #85 is the code beside the message, not the decision to keep the
    /// message.
    ///
    /// The code is [`Self::channel`]'s and not this function's. A read that
    /// re-coded a failure on arrival is exactly how "this version does not
    /// exist" and "this URI is not ours" became one answer.
    pub fn refuse(self) -> ErrorData {
        ErrorData::new(self.channel().code(), redact(&self.message()), None)
    }

    /// Where this failure goes, and under which code.
    ///
    /// The match is exhaustive for the reason [`Self::kind`]'s is: a variant
    /// added without a channel here does not compile, which is the only way a
    /// new failure cannot arrive on somebody else's.
    ///
    /// The three codes are the three remedies, and they are the ones
    /// [`Self::message`] already writes out in prose — "try again" for what is
    /// transient, nothing of the sort for what is not, a narrower request for
    /// what is merely too big. A read's client never sees that prose, so the
    /// code has to carry it or the distinction is lost.
    fn channel(&self) -> Channel {
        match self {
            // The registry answered, and the answer is no. Asking again
            // changes nothing; asking for something else might.
            //
            // `VersionsTooLarge` is here and not with the other two limits,
            // because the remedy and not the cause is what a code carries. A
            // package's release history has no narrower form to ask for —
            // [`Self::message`] says so in as many words — so telling a
            // client to ask for less would send it back with the same call.
            Self::NoSuchPackage { .. }
            | Self::NoSuchVersion { .. }
            | Self::MalformedArchive { .. }
            | Self::UnreadableVersions { .. }
            | Self::VersionsTooLarge { .. }
            | Self::UnreadableSearch { .. }
            | Self::NoSuchFile { .. }
            | Self::PathIsDirectory { .. }
            | Self::UnresolvableArchiveUrl { .. } => Channel::Model(ASK_FOR_SOMETHING_ELSE),

            // Nothing was served. The request was fine and so is the thing it
            // asked about — what failed was the attempt.
            Self::RateLimited { .. }
            | Self::TimedOut { .. }
            | Self::Busy { .. }
            | Self::Unreachable { .. }
            | Self::Unavailable { .. } => Channel::Model(TRY_AGAIN),

            // The thing asked about is there and is over a limit. A narrower
            // request for the same thing is the way to it — a single file
            // instead of a tree, a tighter query, the cursor past one entry.
            Self::TooLarge { .. } | Self::SearchTooLarge { .. } | Self::ItemTooLarge { .. } => {
                Channel::Model(ASK_FOR_LESS)
            }

            // Invalid method parameter(s), in the JSON-RPC specification's own
            // words: a URI that resolves to nothing, a tool nothing answers
            // to, arguments a schema refused. Each is the caller's to fix out
            // of what it was already told.
            Self::InvalidParams { .. } | Self::NoSuchTool { .. } | Self::NoSuchResource { .. } => {
                Channel::Protocol(ErrorCode::INVALID_PARAMS)
            }

            Self::Internal { .. } => Channel::Protocol(INTERNAL_FAILURE),
        }
    }

    /// Which failure this is, in one word, for the line [`crate::log`]
    /// writes.
    ///
    /// Not [`Self::message`] and not `Debug`: a message is a sentence written
    /// for a model and carries the package name a caller sent, so counting by
    /// it would give one bucket per call. This is the cause alone, which is
    /// what "error rate by cause" is a rate of.
    ///
    /// The match is exhaustive on purpose. A variant added without a name
    /// here does not compile, which is the only way a new cause cannot arrive
    /// silently as somebody else's.
    pub fn kind(&self) -> &'static str {
        match self {
            Self::NoSuchPackage { .. } => "no_such_package",
            Self::NoSuchVersion { .. } => "no_such_version",
            Self::RateLimited { .. } => "rate_limited",
            Self::TimedOut { .. } => "timed_out",
            Self::Busy { .. } => "busy",
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
            Self::Internal { .. } => "internal",
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

            Self::Busy { waited } => format!(
                "This server is already downloading as many package archives as it will \
                 hold at once, and no slot came free within {} seconds. Try again shortly.",
                waited.as_secs()
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

            // These four never reach a model — they take the protocol
            // channel, whichever surface they were raised on — but they are
            // still the text a client is shown, and `respond` and `refuse`
            // both take it from here so there is one spelling of each.
            Self::InvalidParams { message } => format!("Invalid parameters: {}", redact(message)),
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
