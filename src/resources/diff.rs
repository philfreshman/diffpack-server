//! `diffpack://diff/{handle}` — a whole comparison, read rather than called.
//!
//! What `get_diff_tree` answers a page at a time, in one document: the
//! inputs, the totals and the tree. The segment is the handle
//! [`crate::handle`] mints and not a `diff_id` — a `diff_id` is a hash, so a
//! URI carrying one could only ever be read out of the cache and would stop
//! resolving the moment an entry was evicted. With the handle the read
//! recomputes instead, which is the same reason `get_diff_tree` takes one.
//! See [ADR 0006](../../docs/adr/0006-the-handle-carries-its-inputs.md).

use rmcp::model::ResourceTemplate;

/// The URI a client fills in to read one comparison.
pub const TEMPLATE: &str = "diffpack://diff/{handle}";

/// The template, as `resources/templates/list` shows it.
pub fn template() -> ResourceTemplate {
    ResourceTemplate::new(TEMPLATE, "diff")
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
