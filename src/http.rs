//! The one HTTP client this crate has.
//!
//! It lives in a module of its own rather than inside the adapter that first
//! needed it, because there is now more than one thing this server fetches: a
//! version's archive ([`crate::archive`]) and what a registry says it
//! publishes ([`crate::catalogue`]). Two clients would be two places to fix a
//! timeout, a user agent or a redirect policy, which is the thing [ADR
//! 0001](../docs/adr/0001-the-archive-seam-is-a-filemap.md) was keeping out
//! of the tools and has the same cost one level down.
//!
//! What leaves this module is a response, bytes, or a [`Failure`] — never a
//! status code. What a status *means* is the caller's, because the two
//! callers mean different things by one: a `404` from an archive URL is a
//! version that does not exist, and a `404` from a search source is the
//! source itself being broken.
//!
//! The module is private to this crate and is not in the list a tool module
//! may import, so the rule that a tool never fetches is unchanged by its
//! existence.
//!
//! # What a request here is allowed to do
//!
//! Reach one of the hosts [`crate::registry`] names, over TLS, once, inside
//! [`error::UPSTREAM_TIMEOUT`], for at most the caller's limit in bytes. A
//! redirect is followed only while it stays on those hosts — a `302` is a
//! request to wherever it points, so a policy that followed one anywhere
//! would be the allowlist with a hole in it that the registry gets to pick.

use std::sync::OnceLock;
use std::time::Duration;

use reqwest::redirect::Policy;
use reqwest::{Client, Response};

use crate::error::{self, Failure};
use crate::registry::{self, Registry};

/// Who is asking, in the header a registry's operator reads when they want to
/// know what this traffic is.
///
/// The version comes from the manifest rather than being written out, so a
/// release cannot leave a stale number in somebody else's logs, and the URL
/// is there because an operator with a question needs somewhere to bring it.
const USER_AGENT: &str = concat!(
    "diffpack-server/",
    env!("CARGO_PKG_VERSION"),
    " (+https://github.com/philfreshman/diffpack-server)",
);

/// Ask `registry` for whatever is at `url`, saying what this will accept.
///
/// The response arrives whatever its status says; mapping a status onto a
/// [`Failure`] is the caller's, because the two callers mean different things
/// by the same number.
///
/// `accept` is a header rather than a constant here because one source needs
/// it to answer at all — PyPI's index is a web page unless a request asks for
/// PEP 691's JSON — and which media type that is belongs to the module that
/// knows what a registry is.
pub async fn get(url: &str, accept: &str, registry: Registry) -> Result<Response, Failure> {
    let name = registry.name();

    // Two budgets, and they are not the same one. The client's timeout
    // covers a request that stalls; `within_budget` covers the whole
    // exchange, so that a body arriving one slow byte at a time still ends
    // inside the time the function has to answer in.
    let request = client()?.get(url).header(reqwest::header::ACCEPT, accept);
    error::within_budget(name, request.send())
        .await?
        .map_err(|cause| failed(cause, name))
}

/// How long the registry asked us to wait, where it said so in seconds.
///
/// The date form of `Retry-After` is not read: it needs a clock and a parser
/// to turn into the number a model is shown, and a registry that sends one
/// still gets "try again shortly", which is the same advice a wrong number
/// would have given.
pub fn retry_after(response: &Response) -> Option<Duration> {
    response
        .headers()
        .get(reqwest::header::RETRY_AFTER)?
        .to_str()
        .ok()?
        .trim()
        .parse::<u64>()
        .ok()
        .map(Duration::from_secs)
}

/// The body, refused as soon as it is known to be over `limit`.
///
/// The declared length is checked before a byte of the body is read, and the
/// running total is checked as each chunk arrives — a body with no
/// `Content-Length`, or one whose header lies, is stopped at the limit rather
/// than after it. The difference matters: the refusal exists so that a
/// package name in a tool argument cannot fill this function's memory, and a
/// cap applied after buffering would have already spent it.
///
/// `too_large` is the caller's, for the reason the status mapping is: what
/// an oversized body *is* differs between an archive nobody can diff and a
/// source answering with more than this server will hold.
pub async fn read_within(
    mut response: Response,
    limit: u64,
    registry: &str,
    too_large: impl Fn(u64) -> Failure,
) -> Result<Vec<u8>, Failure> {
    if let Some(declared) = response.content_length() {
        if declared > limit {
            return Err(too_large(declared));
        }
    }

    let mut body: Vec<u8> = Vec::new();
    while let Some(chunk) = error::within_budget(registry, response.chunk())
        .await?
        .map_err(|cause| failed(cause, registry))?
    {
        let weight = body.len() as u64 + chunk.len() as u64;
        if weight > limit {
            return Err(too_large(weight));
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

/// A request that never became a response.
///
/// The client's own timeout and the budget above it are the same failure to a
/// caller, so they read the same; everything else is the registry not being
/// reachable from here, which is transient and nobody's to fix.
fn failed(cause: reqwest::Error, registry: &str) -> Failure {
    if cause.is_timeout() {
        return Failure::TimedOut {
            registry: registry.to_owned(),
            waited: error::UPSTREAM_TIMEOUT,
        };
    }
    Failure::Unreachable {
        registry: registry.to_owned(),
    }
}

/// The one client, built once.
///
/// A serverless function is built once and invoked many times, so a client
/// per request would be a TLS handshake per request against hosts it just
/// finished talking to. Its configuration is the policy in this module's
/// header: a timeout, a user agent, and redirects that cannot leave the
/// allowlist.
fn client() -> Result<&'static Client, Failure> {
    static CLIENT: OnceLock<Option<Client>> = OnceLock::new();

    CLIENT
        .get_or_init(|| {
            // Which cryptography rustls uses is the depending crate's choice
            // and it has no default, so this is where ours is made: `ring`,
            // because it is the provider that needs no C toolchain at build
            // time and carries no license this repository has not already
            // allowed. Installing it is process-wide and only the first
            // caller wins, which is exactly the intent — a second attempt
            // from a test binary is not a failure.
            let _ = rustls::crypto::ring::default_provider().install_default();

            Client::builder()
                .user_agent(USER_AGENT)
                .timeout(error::UPSTREAM_TIMEOUT)
                .redirect(Policy::custom(|attempt| {
                    if registry::allows(attempt.url().as_str()) {
                        attempt.follow()
                    } else {
                        attempt.stop()
                    }
                }))
                .build()
                .ok()
        })
        .as_ref()
        .ok_or(Failure::Internal {
            doing: "building the HTTP client",
        })
}
