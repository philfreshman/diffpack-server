//! `diffpack://registries` — the registries this server knows, described
//! once.
//!
//! Everything here is read out of [`crate::registry`] rather than written
//! down: the identifiers, the names, where each registry's archives, versions
//! and searches come from, and the name rules an agent would otherwise guess
//! at. See ADR 0004 — a hand-written catalogue beside that module is a copy,
//! and the first URL that changes leaves it telling an agent something
//! untrue.

use rmcp::model::Resource;

/// The URI a client reads this at.
pub const URI: &str = "diffpack://registries";

/// The catalogue, as `resources/list` shows it.
pub fn resource() -> Resource {
    Resource::new(URI, "registries")
        .with_title("Package registries")
        .with_description(
            "The registries this server knows — npm, crates.io and PyPI — with the \
             identifier each is named by in a tool argument, where a version's archive, \
             a package's versions and a search come from, and how each registry spells a \
             package name. Read this once instead of guessing at a scoped npm name or at \
             whether a version range resolves.",
        )
        .with_mime_type("application/json")
}
