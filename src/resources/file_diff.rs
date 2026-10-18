use std::borrow::Cow;

use rmcp::model::{CacheScope, ReadResourceResult, ResourceContents, ResourceTemplate};

use crate::engine::DiffFileEntry;
use crate::error::Failure;
use crate::handle::DiffHandle;
use crate::tools::get_file_diff::{self, OneFile};
use crate::tools::{diff_package_versions, get_diff_tree, Call};

const SEPARATOR: &str = "/file/";

pub fn uri_template() -> String {
    format!("{}{{handle}}{SEPARATOR}{{path}}", super::diff::PREFIX)
}

pub fn parts_in(uri: &str) -> Option<(&str, &str)> {
    let rest = uri.strip_prefix(super::diff::PREFIX)?;
    let (handle, path) = rest.split_once(SEPARATOR)?;

    (!handle.is_empty() && !handle.contains('/') && !path.is_empty()).then_some((handle, path))
}

pub fn template() -> ResourceTemplate {
    ResourceTemplate::new(uri_template(), "file-diff")
        .with_title("File diff")
        .with_description(
            "One file's diff out of a comparison, with three lines of unchanged context \
             around each change. `{handle}` is the handle `diff_package_versions` gave \
             you and `{path}` is a path from that comparison's tree, with the archive's \
             top-level directory already removed. A `text/x-diff` answer is a patch; a \
             `text/plain` one is not — a file both versions ship unchanged comes back as \
             itself, and a path neither version has comes back as a sentence saying so.",
        )
        .with_mime_type("text/x-diff")
}

const TTL_MS: u64 = 24 * 60 * 60 * 1000;

pub async fn read(
    handle: &DiffHandle,
    path: &str,
    call: &Call,
) -> Result<ReadResourceResult, Failure> {
    let wanted = decoded(path)?;

    let comparison = diff_package_versions::compare(handle, call).await?;

    let moved = moved_from(&comparison.tree, &wanted);
    let asked = OneFile {
        path: &wanted,
        old_path: moved.as_deref(),
        context_lines: get_file_diff::ContextLines::default(),
        max_bytes: None,
    };

    let patch = comparison.file_patch(call, asked).await?;

    let contents = ResourceContents::text(
        patch.excerpt.text,
        format!("{}{SEPARATOR}{path}", super::diff::uri_of(handle)),
    )
    .with_mime_type(if patch.is_diff {
        "text/x-diff"
    } else {
        "text/plain"
    });

    Ok(ReadResourceResult::new(vec![contents])
        .with_ttl_ms(TTL_MS)
        .with_cache_scope(CacheScope::Public))
}

fn decoded(segment: &str) -> Result<Cow<'_, str>, Failure> {
    if !segment.contains('%') {
        return Ok(Cow::Borrowed(segment));
    }

    let raw = segment.as_bytes();
    let mut bytes = Vec::with_capacity(raw.len());
    let mut at = 0;

    while at < raw.len() {
        let byte = raw[at];

        if byte != b'%' {
            bytes.push(byte);
            at += 1;
            continue;
        }

        let (Some(high), Some(low)) = (
            raw.get(at + 1).and_then(|&digit| hex(digit)),
            raw.get(at + 2).and_then(|&digit| hex(digit)),
        ) else {
            return Err(malformed(segment));
        };

        bytes.push(high * 16 + low);
        at += 3;
    }

    String::from_utf8(bytes)
        .map(Cow::Owned)
        .map_err(|_| malformed(segment))
}

fn hex(digit: u8) -> Option<u8> {
    char::from(digit)
        .to_digit(16)
        .and_then(|value| u8::try_from(value).ok())
}

fn malformed(segment: &str) -> Failure {
    Failure::InvalidParams {
        message: format!(
            "`{segment}` is not a path this server can read: a `%` in a URI introduces \
             two hexadecimal digits standing for one byte, and these spell no text. Pass \
             the path as the comparison's tree gives it, percent-encoded or not."
        ),
    }
}

fn moved_from(tree: &DiffFileEntry, path: &str) -> Option<String> {
    get_diff_tree::node_at(tree, path, get_diff_tree::NodeType::File)?
        .old_path
        .clone()
}
