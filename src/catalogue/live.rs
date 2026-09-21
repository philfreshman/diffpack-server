//! What a registry publishes, asked of the registry.
//!
//! What this module knows is what a status means when the thing being asked
//! for is a catalogue rather than an archive — which is not the same mapping.
//! A `404` here is not "no such package": nothing in the URL names a package,
//! so a search source answering `404` is a source that has moved or broken,
//! and the remedy is to try again or to search elsewhere rather than to check
//! a spelling.

use reqwest::{Response, StatusCode};

use crate::error::Failure;
use crate::http;
use crate::registry::Registry;

/// The adapter that asks.
#[derive(Debug)]
pub struct Live;

impl Live {
    pub fn new() -> Self {
        Self
    }

    /// Whatever `url` serves, as text, refusing anything over `limit`.
    pub async fn body(&self, url: &str, limit: u64, registry: Registry) -> Result<String, Failure> {
        let name = registry.name();
        let response = success(http::get(url, registry).await?, registry)?;

        let bytes = http::read_within(response, limit, name, |_| Failure::Unavailable {
            registry: name.to_owned(),
            // A body this server will not hold is not an HTTP status, and
            // there is no status to report because the answer arrived. `0`
            // is the one number that cannot be mistaken for one a registry
            // sent, and the message a model reads is the same either way:
            // this source answered with something unusable.
            status: 0,
        })
        .await?;

        String::from_utf8(bytes).map_err(|_| Failure::Unavailable {
            registry: name.to_owned(),
            status: 0,
        })
    }
}

/// The response, or the failure its status names.
///
/// Every arm is transient or nearly so, which is the difference from the
/// archive path: there is no argument here that could have been wrong, so
/// there is no remedy that involves the caller changing one. A search that
/// fails says "try again", and says which registry it was.
fn success(response: Response, registry: Registry) -> Result<Response, Failure> {
    let status = response.status();
    if status.is_success() {
        return Ok(response);
    }

    Err(match status {
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
