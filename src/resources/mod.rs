//! One module per resource.
//!
//! A [`Resource`](rmcp::model::Resource) is what an agent *reads* where a
//! tool is what it *calls*: a URI, and the answer at the other end. The
//! collection here is the resources half of what [`crate::tools`] is for
//! tools — the list a client is told, and the dispatch that answers one URI
//! — and each resource's URI, its description and its handler live together
//! in one module, for the reason ADR 0002 gives for a tool.
//!
//! # Why a template is not a resource
//!
//! The two lists are two methods, because the `2026-07-28` schema gives them
//! two shapes: a [`Resource`] carries a `uri` a client can follow as it
//! stands, and a [`ResourceTemplate`] carries a `uriTemplate` with a field to
//! fill in first. `diffpack://registries` is the first and the two diffs are
//! the second, so [`catalogue`] answers `resources/list` and [`templates`]
//! answers `resources/templates/list`. A template listed as a resource would
//! be a URI a client followed literally and got `-32602` for.

pub mod diff;
pub mod file_diff;
pub mod registries;

use rmcp::model::{ReadResourceResult, Resource, ResourceTemplate};

use crate::error::Failure;
use crate::handle::DiffHandle;
use crate::tools::Call;

/// Every resource a client can read by name, in a fixed order.
pub fn catalogue() -> Vec<Resource> {
    vec![registries::resource()]
}

/// Every resource a client reads by filling a URI in, in a fixed order.
///
/// The whole comparison before one file of it, which is the order they are
/// read in: a caller finds the path in the first and asks for it in the
/// second.
pub fn templates() -> Vec<ResourceTemplate> {
    vec![diff::template(), file_diff::template()]
}

/// Read the resource `uri` names, or refuse a URI that is not one of ours.
///
/// Each module matches its own URI, so there is no table here to keep in step
/// with the templates above — and no order to get wrong: a handle is base64url
/// behind a version prefix and carries no `/`, so the URI with a path in it
/// and the URI without one are told apart by their own shapes rather than by
/// which matcher was tried first.
///
/// A [`Failure::NoSuchResource`] is `-32602` on the protocol channel: a URI
/// that resolves to nothing is something a client built, and there is no
/// resource to have failed. A handle that does not decode is the same code by
/// a different route — [`DiffHandle::decode`] refuses it — which is what
/// keeps the refusal in one module rather than in each resource that takes
/// one.
pub async fn read(uri: &str, call: &Call) -> Result<ReadResourceResult, Failure> {
    if uri == registries::URI {
        return registries::read();
    }

    if let Some((handle, path)) = file_diff::parts_in(uri) {
        return file_diff::read(&DiffHandle::decode(handle)?, path, call).await;
    }

    if let Some(handle) = diff::handle_in(uri) {
        return diff::read(&DiffHandle::decode(handle)?, call).await;
    }

    Err(Failure::NoSuchResource {
        uri: uri.to_owned(),
    })
}
