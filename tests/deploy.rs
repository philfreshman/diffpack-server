//! `vercel.json` is the only place the deployment shape is written down, and
//! nothing in the crate compiles against it — so a rename on either side is
//! invisible until a deploy serves a 404.
//!
//! These tests hold the two artifacts together the way `tests/engine.rs`
//! holds `Cargo.toml`'s pinned tag against `engine::VERSION`: the manifest
//! and the crate layout have to agree, and disagreeing must fail here rather
//! than in production.
//!
//! What they deliberately do not do is read a value out of `vercel.json` and
//! assert it back. The branch policy and `maxDuration` are verified against
//! the deployed result (#8), because a test that restates the file cannot
//! disagree with it.

use std::path::Path;

/// The repository root. Tests run with an unspecified working directory, so
/// the path has to come from cargo rather than from `.`.
fn repo_root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
}

fn vercel_json() -> serde_json::Value {
    let path = repo_root().join("vercel.json");
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("vercel.json should exist at {}: {e}", path.display()));
    serde_json::from_str(&text).expect("vercel.json should parse")
}

/// Vercel's Rust runtime discovers handlers by finding `api/*.rs`, and
/// `vercel.json` names one of them again in `functions` to give it a
/// `maxDuration` — the only place a Rust handler can declare one. Two names
/// for one file is a rename waiting to go wrong, so they are checked against
/// each other and against the file on disk.
#[test]
fn the_function_vercel_json_configures_is_the_one_cargo_builds() {
    let manifest = vercel_json();

    let configured = manifest["functions"]
        .as_object()
        .expect("vercel.json should have a `functions` object")
        .keys()
        .next()
        .expect("`functions` should name the handler")
        .clone();

    assert!(
        repo_root().join(&configured).is_file(),
        "vercel.json configures `{configured}`, which is not a file in this repository"
    );

    let cargo: toml::Value =
        toml::from_str(include_str!("../Cargo.toml")).expect("Cargo.toml should parse");
    let built = cargo["bin"][0]["path"]
        .as_str()
        .expect("Cargo.toml should declare the handler as a [[bin]] with a path");

    assert_eq!(
        configured, built,
        "vercel.json configures `{configured}` but Cargo.toml builds `{built}`"
    );
}

/// Routing lives inside the function, not in the platform: `api/mcp.rs`
/// matches on the path itself so that `/health` and (in #6) `/mcp` are one
/// binary's business. That only holds if every path actually arrives, which
/// is what the catch-all rewrite is for. A rewrite pointing somewhere the
/// handler is not would strand every request.
#[test]
fn every_path_is_rewritten_to_the_handler() {
    let manifest = vercel_json();

    let rewrites = manifest["rewrites"]
        .as_array()
        .expect("vercel.json should have a `rewrites` array");

    let catch_all = rewrites
        .iter()
        .find(|r| r["source"] == "/(.*)")
        .expect("a `/(.*)` rewrite should send every path to the handler");

    let destination = catch_all["destination"]
        .as_str()
        .expect("the rewrite should have a destination");

    let handler = manifest["functions"]
        .as_object()
        .and_then(|f| f.keys().next())
        .expect("`functions` should name the handler");

    // `api/mcp.rs` is deployed at `/api/mcp`: Vercel strips the extension and
    // serves the handler at its path under the repository root.
    let expected = format!("/{}", handler.trim_end_matches(".rs"));

    assert_eq!(
        destination, expected,
        "the rewrite sends every path to `{destination}`, but the handler is deployed at `{expected}`"
    );
}
