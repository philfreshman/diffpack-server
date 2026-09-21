//! Archives from the registries themselves.
//!
//! What this module knows is what a registry's *status* means when the thing
//! being asked for is a version's archive: a `404` is a version that does not
//! exist, and a body over the cap is a package too large to diff. The client
//! that makes the request is [`crate::http`]'s, because a version's archive
//! is no longer the only thing this server fetches ([ADR
//! 0001](../docs/adr/0001-the-archive-seam-is-a-filemap.md)).

use reqwest::{Response, StatusCode};

use crate::error::Failure;
use crate::http;
use crate::registry::Registry;

/// The adapter that fetches.
#[derive(Debug)]
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
        let name = registry.name();
        let response = success(http::get(url, registry).await?, registry, package, version)?;

        http::read_within(response, limit, name, |bytes| {
            super::too_large(package, version, bytes, limit)
        })
        .await
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
        // What a registry answers for an archive that is not there.
        // `super::not_found` is where the reasoning is, and it is the
        // fixture adapter's refusal too.
        StatusCode::NOT_FOUND | StatusCode::FORBIDDEN | StatusCode::GONE => {
            super::not_found(registry, package, version)
        }

        StatusCode::TOO_MANY_REQUESTS => Failure::RateLimited {
            registry: registry.name().to_owned(),
            retry_after: http::retry_after(&response),
        },

        other => Failure::Unavailable {
            registry: registry.name().to_owned(),
            status: other.as_u16(),
        },
    })
}
