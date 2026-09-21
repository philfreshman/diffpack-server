//! Version lists from the registries themselves.
//!
//! The counterpart of [`crate::archive::live`], and as thin: the client is
//! [`crate::fetch`]'s, and what is here is the part that is about *version
//! lists* — a `404` means the registry has no such package, and a body over
//! the cap is a document this server declined to read.

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
    /// `registry` and `package` are here for the message rather than for the
    /// request: a failure a model can act on says which registry went wrong
    /// and what was being asked for.
    pub async fn bytes(
        &self,
        url: &str,
        limit: u64,
        registry: Registry,
        package: &str,
    ) -> Result<Vec<u8>, Failure> {
        fetch::bytes(
            url,
            limit,
            &About {
                registry,
                missing: &|| super::no_such_package(registry, package),
                too_large: &|bytes| super::too_large(registry, package, bytes, limit),
            },
        )
        .await
    }
}
