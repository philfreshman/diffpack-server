use rmcp::model::{CacheScope, ReadResourceResult, Resource, ResourceContents, ResourceTemplate};
use serde::Serialize;

use crate::error::Failure;
use crate::handle::{DiffHandle, Inputs};
use crate::page;
use crate::tools::get_diff_tree::{self, Node};
use crate::tools::{diff_package_versions, Call, Tool};

pub const PREFIX: &str = "diffpack://diff/";

pub fn uri_template() -> String {
    format!("{PREFIX}{{handle}}")
}

pub fn handle_in(uri: &str) -> Option<&str> {
    let handle = uri.strip_prefix(PREFIX)?;
    (!handle.is_empty() && !handle.contains('/')).then_some(handle)
}

pub fn uri_of(handle: &DiffHandle) -> String {
    format!("{PREFIX}{}", handle.encode())
}

pub fn link(handle: &DiffHandle) -> Resource {
    Resource::new(uri_of(handle), "diff")
        .with_title("This comparison")
        .with_description(
            "The whole of the comparison this call made: what was compared, how much \
             changed, and every file and directory in it. Reading it is the same answer \
             `get_diff_tree` pages through.",
        )
        .with_mime_type("application/json")
}

pub fn template() -> ResourceTemplate {
    ResourceTemplate::new(uri_template(), "diff")
        .with_title("Package comparison")
        .with_description(
            "One comparison of two published versions: what was compared, how much \
             changed, and every file and directory in it. `{handle}` is the handle \
             `diff_package_versions` gave you, passed back exactly as it arrived. A \
             comparison too large to serve whole comes back as its totals with a pointer \
             to `get_diff_tree`, which pages through the same tree.",
        )
        .with_mime_type("application/json")
}

const TTL_MS: u64 = 24 * 60 * 60 * 1000;

pub async fn read(handle: &DiffHandle, call: &Call) -> Result<ReadResourceResult, Failure> {
    let tree = diff_package_versions::compare(handle, call).await?.tree;
    let totals = diff_package_versions::totals(&tree);

    let whole = write(&Document {
        inputs: handle.inputs(),
        totals: &totals,
        tree: Some(get_diff_tree::nodes(&tree)),
        tree_too_large: None,
    })?;

    let text = if page::fits(&whole) {
        whole
    } else {
        write(&Document {
            inputs: handle.inputs(),
            totals: &totals,
            tree: None,
            tree_too_large: Some(TooLarge::at(whole.len())),
        })?
    };

    let contents = ResourceContents::text(text, uri_of(handle)).with_mime_type("application/json");

    Ok(ReadResourceResult::new(vec![contents])
        .with_ttl_ms(TTL_MS)
        .with_cache_scope(CacheScope::Public))
}

#[derive(Serialize)]
struct Document<'d> {
    inputs: &'d Inputs,

    totals: &'d diff_package_versions::Totals,

    #[serde(skip_serializing_if = "Option::is_none")]
    tree: Option<Vec<Node>>,

    #[serde(skip_serializing_if = "Option::is_none")]
    tree_too_large: Option<TooLarge>,
}

#[derive(Serialize)]
struct TooLarge {
    bytes: usize,

    ceiling: usize,

    read_with: &'static str,

    note: String,
}

impl TooLarge {
    fn at(bytes: usize) -> Self {
        Self {
            bytes,
            ceiling: page::PAYLOAD_CEILING,
            read_with: TREE_TOOL,
            note: format!(
                "This comparison's tree is {bytes} bytes and one answer carries at most \
                 {}, so it is not in this document — none of it, rather than as much as \
                 fits, because part of a tree reads exactly like all of one. Call \
                 `{TREE_TOOL}` with the same handle to walk it a page at a time; the \
                 totals above are the whole comparison's either way.",
                page::PAYLOAD_CEILING,
            ),
        }
    }
}

const TREE_TOOL: &str = <get_diff_tree::GetDiffTree as Tool>::NAME;

fn write(document: &Document<'_>) -> Result<String, Failure> {
    serde_json::to_string_pretty(document).map_err(|_| Failure::Internal {
        doing: "answering a resource read",
    })
}
