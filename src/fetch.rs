//! The HTTP client this crate has, and the only one.
//!
//! It was [`crate::archive`]'s until there was a second thing to fetch. What
//! belongs here is what every outbound request shares whatever it is asking
//! for: one client built once, a user agent, a timeout, redirects that cannot
//! leave the host allowlist, and a body read no further than a caller's limit.
//! [ADR 0001](../docs/adr/0001-the-archive-seam-is-a-filemap.md)'s reasoning
//! is the reason this is one module rather than one per seam — eight callers
//! that each know how to fetch is eight places to fix a timeout — and the
//! reason the seams sit *above* it: what leaves this module is bytes or a
//! [`Failure`], never a status code and never a `reqwest` type.
//!
//! # What a request here is allowed to do
//!
//! Reach one of the hosts [`crate::registry`] names, over TLS, once, inside
//! [`error::UPSTREAM_TIMEOUT`], for at most the caller's limit in bytes. A
//! redirect is followed only while it stays on those hosts — a `302` is a
//! request to wherever it points, so a policy that followed one anywhere
//! would be the allowlist with a hole in it that the registry gets to pick.
//!
//! # Why the caller supplies the refusals
//!
//! A `404` means something different to each seam: for an archive it is a
//! version that does not exist, for a version listing it is a package that
//! does not. Neither is this module's to name, and a single "not found" for
//! both would be a message a model cannot act on. So a caller passes an
//! [`About`] describing what it is asking for, and this module builds the
//! rest — a rate limit, a timeout, a body over the cap — the same way for
//! everyone.

use std::sync::OnceLock;

use reqwest::redirect::Policy;
use reqwest::{Client, Response, StatusCode};

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

/// What a request is for, in the words its failures need.
///
/// Carried rather than derived because this module cannot tell an archive
/// from a listing by looking at a URL, and the two fail differently. The two
/// closures are the caller's answers to the only two questions that differ:
/// what a registry saying "no such thing" means, and what to call a body this
/// server will not hold.
pub struct About<'a> {
    /// The registry being asked, for the messages that name it.
    pub registry: Registry,

    /// What to return when the registry says there is no such thing —
    /// a `404`, a `403` or a `410`.
    pub missing: &'a (dyn Fn() -> Failure + Send + Sync),

    /// What to return when the body is larger than `limit`, given its weight.
    pub too_large: &'a (dyn Fn(u64) -> Failure + Send + Sync),
}

/// Whatever `url` serves, as bytes, refusing anything over `limit`.
pub async fn bytes(url: &str, limit: u64, about: &About<'_>) -> Result<Vec<u8>, Failure> {
    let client = client()?;
    let name = about.registry.name();

    // Two budgets, and they are not the same one. The client's timeout
    // covers a request that stalls; `within_budget` covers the whole
    // exchange, so that a body arriving one slow byte at a time still
    // ends inside the time the function has to answer in.
    let response = error::within_budget(name, client.get(url).send())
        .await?
        .map_err(|cause| failed(cause, name))?;

    let response = success(response, about)?;
    read_within(response, limit, name, about).await
}

/// The response, or the failure its status names.
///
/// Every arm is a different remedy, which is the point: a model told to slow
/// down waits, a model told the version does not exist asks for one that
/// does, and a model told the registry is unwell tries again later. A single
/// "HTTP error" would leave it guessing which of those it is looking at.
fn success(response: Response, about: &About<'_>) -> Result<Response, Failure> {
    let status = response.status();
    if status.is_success() {
        return Ok(response);
    }

    Err(match status {
        // What a registry answers for something that is not there. Which
        // "something" is the caller's to say — see the module header.
        StatusCode::NOT_FOUND | StatusCode::FORBIDDEN | StatusCode::GONE => (about.missing)(),

        StatusCode::TOO_MANY_REQUESTS => Failure::RateLimited {
            registry: about.registry.name().to_owned(),
            retry_after: retry_after(&response),
        },

        other => Failure::Unavailable {
            registry: about.registry.name().to_owned(),
            status: other.as_u16(),
        },
    })
}

/// How long the registry asked us to wait, where it said so in seconds.
///
/// The date form of `Retry-After` is not read: it needs a clock and a parser
/// to turn into the number a model is shown, and a registry that sends one
/// still gets "try again shortly", which is the same advice a wrong number
/// would have given.
fn retry_after(response: &Response) -> Option<std::time::Duration> {
    response
        .headers()
        .get(reqwest::header::RETRY_AFTER)?
        .to_str()
        .ok()?
        .trim()
        .parse::<u64>()
        .ok()
        .map(std::time::Duration::from_secs)
}

/// The body, refused as soon as it is known to be over `limit`.
///
/// The declared length is checked before a byte of the body is read, and the
/// running total is checked as each chunk arrives — a body with no
/// `Content-Length`, or one whose header lies, is stopped at the limit rather
/// than after it. The difference matters: the refusal exists so that a
/// package name in a tool argument cannot fill this function's memory, and a
/// cap applied after buffering would have already spent it.
async fn read_within(
    mut response: Response,
    limit: u64,
    registry: &str,
    about: &About<'_>,
) -> Result<Vec<u8>, Failure> {
    if let Some(declared) = response.content_length() {
        if declared > limit {
            return Err((about.too_large)(declared));
        }
    }

    let mut body: Vec<u8> = Vec::new();
    while let Some(chunk) = error::within_budget(registry, response.chunk())
        .await?
        .map_err(|cause| failed(cause, registry))?
    {
        let weight = body.len() as u64 + chunk.len() as u64;
        if weight > limit {
            return Err((about.too_large)(weight));
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
