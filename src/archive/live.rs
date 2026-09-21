//! Archives from the registries themselves.
//!
//! What is left here after [`crate::fetch`] took the client is the part that
//! is about *archives*: a `404` from a registry means the version does not
//! exist, and a body over the cap is an archive this server declined to hold.
//! Both refusals are built by [`super`], so the fixture adapter cannot answer
//! differently from this one.

use crate::error::Failure;
use crate::fetch::{self, About};
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
        fetch::bytes(
            url,
            limit,
            &About {
                registry,
                missing: &|| super::not_found(registry, package, version),
                too_large: &|bytes| super::too_large(package, version, bytes, limit),
            },
        )
        .await
    }
}
