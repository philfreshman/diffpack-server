//! Archives from the registries themselves.
//!
//! The HTTP client this crate has, and the only one: it lives here because
//! eight tools that each know how to fetch is eight places to fix a timeout,
//! a user agent or a retry ([ADR
//! 0001](../docs/adr/0001-the-archive-seam-is-a-filemap.md)). What leaves
//! this module is bytes or a [`Failure`], never a status code.
//!
//! # What a request here is allowed to do
//!
//! Reach one of the hosts [`crate::registry`] names, over TLS, once, inside
//! [`error::UPSTREAM_TIMEOUT`], for at most the caller's limit in bytes. A
//! redirect is followed only while it stays on those hosts — a `302` is a
//! request to wherever it points, so a policy that followed one anywhere
//! would be the allowlist with a hole in it that the registry gets to pick.

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

/// The adapter that fetches.
pub struct Live;

impl Live {
    pub fn new() -> Self {
        Self
    }

    /// Whatever `url` serves, as bytes, refusing anything over `limit`.
    ///
    /// `registry`, `package` and `version` are here for the message rather
    /// than for the request: a failure a model can act on says which registry
    /// went wrong and what was being asked for, and this is the only place
    /// that knows both.
    pub async fn bytes(
        &self,
        url: &str,
        limit: u64,
        registry: Registry,
        package: &str,
        version: &str,
    ) -> Result<Vec<u8>, Failure> {
        let client = client()?;
        let name = registry.name();

        // Two budgets, and they are not the same one. The client's timeout
        // covers a request that stalls; `within_budget` covers the whole
        // exchange, so that a body arriving one slow byte at a time still
        // ends inside the time the function has to answer in.
        let response = error::within_budget(name, client.get(url).send())
            .await?
            .map_err(|cause| failed(cause, name))?;

        let response = success(response, registry, package, version)?;
        read_within(response, limit, name, package, version).await
    }
}

/// The response, or the failure its status names.
///
/// Every arm is a different remedy, which is the point: a model told to slow
/// down waits, a model told the version does not exist asks for one that
/// does, and a model told the registry is unwell tries again later. A single
/// "HTTP error" would leave it guessing which of those it is looking at.
fn success(
    response: Response,
    registry: Registry,
    package: &str,
    version: &str,
) -> Result<Response, Failure> {
    let status = response.status();
    if status.is_success() {
        return Ok(response);
    }

    Err(match status {
        // What a registry answers for an archive that is not there. Which
        // half is wrong — the package or the version — is not in the status,
        // and a version is the far commoner mistake; #18 is what will let
        // this message carry the versions that do exist.
        StatusCode::NOT_FOUND | StatusCode::FORBIDDEN | StatusCode::GONE => {
            Failure::NoSuchVersion {
                registry: registry.name().to_owned(),
                package: package.to_owned(),
                version: version.to_owned(),
                known: Vec::new(),
            }
        }

        StatusCode::TOO_MANY_REQUESTS => Failure::RateLimited {
            registry: registry.name().to_owned(),
            retry_after: retry_after(&response),
        },

        other => Failure::Unavailable {
            registry: registry.name().to_owned(),
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
    package: &str,
    version: &str,
) -> Result<Vec<u8>, Failure> {
    let too_large = |bytes| super::too_large(package, version, bytes, limit);

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
