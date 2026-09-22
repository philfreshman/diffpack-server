//! `diffpack://diff/{handle}/file/{path}` — one file's diff out of a
//! comparison.
//!
//! The same bytes [`get_file_diff`](crate::tools::get_file_diff) returns at
//! its defaults, which is three lines of unchanged context around each
//! change. What that tool carries in a field, this carries in a media type: a
//! patch is `text/x-diff` and the two answers that are not patches — a file
//! both versions ship byte for byte, and a path neither version has — are
//! `text/plain`, so a client renders a file as a file without a field to read
//! first.

use rmcp::model::ResourceTemplate;

/// The URI a client fills in to read one file's diff.
pub const TEMPLATE: &str = "diffpack://diff/{handle}/file/{path}";

/// The template, as `resources/templates/list` shows it.
pub fn template() -> ResourceTemplate {
    ResourceTemplate::new(TEMPLATE, "file-diff")
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
