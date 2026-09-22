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
/// A [`Failure::NoSuchResource`] is `-32602` on the protocol channel: a URI
/// that resolves to nothing is something a client built, and there is no
/// resource to have failed.
pub fn read(uri: &str) -> Result<ReadResourceResult, Failure> {
    match uri {
        registries::URI => Ok(registries::read()),
        unknown => Err(Failure::NoSuchResource {
            uri: unknown.to_owned(),
        }),
    }
}
