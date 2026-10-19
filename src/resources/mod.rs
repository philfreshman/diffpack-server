pub mod diff;
pub mod file_diff;
pub mod registries;

use rmcp::model::{JsonObject, ReadResourceResult, Resource, ResourceTemplate};
use serde_json::Value;

use crate::error::Failure;
use crate::handle::DiffHandle;
use crate::tools::Call;

pub fn catalogue() -> Vec<Resource> {
    vec![registries::resource()]
}

pub fn templates() -> Vec<ResourceTemplate> {
    vec![diff::template(), file_diff::template()]
}

const PATH: &str = "path";

#[derive(Debug, Clone, Copy)]
pub enum Address<'a> {
    Registries,

    Diff { handle: &'a str },

    FileDiff { handle: &'a str, path: &'a str },

    Nowhere { uri: &'a str },
}

impl<'a> Address<'a> {
    pub fn of(uri: &'a str) -> Self {
        if uri == registries::URI {
            return Self::Registries;
        }

        if let Some((handle, path)) = file_diff::parts_in(uri) {
            return Self::FileDiff { handle, path };
        }

        if let Some(handle) = diff::handle_in(uri) {
            return Self::Diff { handle };
        }

        Self::Nowhere { uri }
    }

    pub fn resource(self) -> &'a str {
        match self {
            Self::Registries => registries::RESOURCE_NAME,
            Self::Diff { .. } => diff::RESOURCE_NAME,
            Self::FileDiff { .. } => file_diff::RESOURCE_NAME,
            Self::Nowhere { uri } => uri,
        }
    }

    pub fn arguments(self) -> JsonObject {
        match self {
            Self::Diff { handle } => inputs_named_in(handle),
            Self::FileDiff { handle, path } => {
                let mut arguments = inputs_named_in(handle);
                arguments.insert(PATH.to_owned(), Value::from(path));
                arguments
            }
            Self::Registries | Self::Nowhere { .. } => JsonObject::new(),
        }
    }

    pub async fn read(self, call: &Call) -> Result<ReadResourceResult, Failure> {
        match self {
            Self::Registries => registries::read(),
            Self::Diff { handle } => diff::read(&DiffHandle::decode(handle)?, call).await,
            Self::FileDiff { handle, path } => {
                file_diff::read(&DiffHandle::decode(handle)?, path, call).await
            }
            Self::Nowhere { uri } => Err(Failure::NoSuchResource {
                uri: uri.to_owned(),
            }),
        }
    }
}

fn inputs_named_in(handle: &str) -> JsonObject {
    DiffHandle::decode(handle)
        .ok()
        .and_then(|decoded| serde_json::to_value(decoded.inputs()).ok())
        .and_then(|inputs| inputs.as_object().cloned())
        .unwrap_or_default()
}
