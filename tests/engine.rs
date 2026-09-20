//! `src/engine.rs` is the only module in this crate allowed to name
//! `diffpack_engine`. These tests hold that boundary and hold the version
//! constant honest.

use diffpack_server::engine;

/// `engine::VERSION` is a field in the cache key (#5): a new engine release
/// can change rename detection or line counts, and a cache that served the
/// old tree afterwards would be serving a diff the current engine would not
/// produce. So the constant is not documentation — a stale one is a
/// correctness bug, and bumping the dependency without bumping it must fail
/// here rather than in production.
#[test]
fn the_version_constant_matches_the_pinned_dependency() {
    let manifest: toml::Value =
        toml::from_str(include_str!("../Cargo.toml")).expect("Cargo.toml should parse");

    let tag = manifest["dependencies"]["diffpack-engine"]["tag"]
        .as_str()
        .expect("diffpack-engine should be pinned to a tag, not a branch or a rev");

    assert_eq!(
        tag,
        format!("v{}", engine::VERSION),
        "Cargo.toml pins {tag} but engine::VERSION says {}",
        engine::VERSION
    );
}

/// Proves the seam is usable and not merely re-exported: the types line up,
/// the signature is the one we think it is, and a build against a new engine
/// tag that moved either fails here.
///
/// No network and no archive — `build_diff_tree` takes file maps, so the
/// smallest honest exercise of it is two maps built by hand.
#[test]
fn the_tree_builder_is_reachable_through_the_seam() {
    use diffpack_server::engine::{build_diff_tree, DiffStatus, FileMapEntry, FileType};
    use std::collections::HashMap;

    let file = |content: &str| FileMapEntry {
        file_type: FileType::File,
        content: content.to_string(),
    };

    let from = HashMap::from([("a.txt".to_string(), file("one\ntwo\n"))]);
    let to = HashMap::from([("a.txt".to_string(), file("one\nTWO\n"))]);

    let tree = build_diff_tree(&from, &to, 0.75, false);
    let changed = find_path(&tree, "a.txt").expect("a.txt should be in the tree");

    assert_eq!(changed.status, DiffStatus::Modified);
    assert_eq!(changed.added, Some(1));
    assert_eq!(changed.removed, Some(1));
}

fn find_path(
    entry: &diffpack_server::engine::DiffFileEntry,
    path: &str,
) -> Option<diffpack_server::engine::DiffFileEntry> {
    if entry.path == path {
        return Some(entry.clone());
    }
    entry
        .children
        .as_ref()?
        .iter()
        .find_map(|child| find_path(child, path))
}

/// `extract_archive_bytes` reaches the seam too. Rejecting rubbish is the
/// cheapest call that proves the signature without shipping a fixture
/// archive; #10 is where real archives arrive.
#[test]
fn the_archive_extractor_is_reachable_through_the_seam() {
    let result = diffpack_server::engine::extract_archive_bytes(b"not an archive");
    assert!(result.is_err(), "rubbish bytes should not extract");
}
