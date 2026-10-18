pub mod diff;
pub mod file_diff;
pub mod registries;

use rmcp::model::{ReadResourceResult, Resource, ResourceTemplate};

use crate::error::Failure;
use crate::handle::DiffHandle;
use crate::tools::Call;

pub fn catalogue() -> Vec<Resource> {
    vec![registries::resource()]
}

pub fn templates() -> Vec<ResourceTemplate> {
    vec![diff::template(), file_diff::template()]
}

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
