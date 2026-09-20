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

/// The unified-diff format, pinned byte for byte.
///
/// This is the one piece of the engine whose *output* is the contract rather
/// than just its signature. The server renders file views from it and the
/// tree's counts come from the same lines, so a byte that moves here makes
/// the two disagree — and the alternative to depending on the engine was
/// re-implementing this format, which is exactly the drift the seam exists to
/// prevent.
///
/// The expectations below are written from the format the engine documents —
/// a `--- from/{f}` / `+++ to/{f}` header, then one line per change as sign,
/// a space, and the line with any trailing newline removed — not by running
/// the function and recording what it said.
#[test]
fn the_unified_diff_format_is_byte_for_byte_what_the_engine_documents() {
    use diffpack_server::engine::get_diff_content;

    let cases: [(&str, &str, &str, &str); 4] = [
        (
            "an added file",
            "",
            "one\ntwo\n",
            "--- from/a.txt\n+++ to/a.txt\n+ one\n+ two",
        ),
        (
            "a removed file",
            "one\ntwo\n",
            "",
            "--- from/a.txt\n+++ to/a.txt\n- one\n- two",
        ),
        (
            "a modified file",
            "one\ntwo\n",
            "one\nTWO\n",
            "--- from/a.txt\n+++ to/a.txt\n  one\n- two\n+ TWO",
        ),
        (
            "a byte-identical file",
            "one\ntwo\n",
            "one\ntwo\n",
            "--- from/a.txt\n+++ to/a.txt\n  one\n  two",
        ),
    ];

    for (what, from, to, expected) in cases {
        let actual = get_diff_content("a.txt", from, to, false);
        assert_eq!(actual, expected, "{what}");
    }
}

/// `ignore_whitespace` reaches the renderer, and the two modes are the two
/// `similar` values rather than a local re-spelling of them.
#[test]
fn the_whitespace_setting_is_reachable_and_changes_the_diff() {
    use diffpack_server::engine::{get_diff_content, whitespace_mode, WhitespaceMode};

    assert_eq!(whitespace_mode(false), WhitespaceMode::Exact);
    assert_eq!(whitespace_mode(true), WhitespaceMode::IgnoreAll);

    let (from, to) = ("one\ntwo\n", "one\n  two  \n");

    assert_eq!(
        get_diff_content("a.txt", from, to, true),
        "--- from/a.txt\n+++ to/a.txt\n  one\n    two  ",
        "ignoring whitespace, the second line is unchanged"
    );
    assert_eq!(
        get_diff_content("a.txt", from, to, false),
        "--- from/a.txt\n+++ to/a.txt\n  one\n- two\n+   two  ",
        "exactly, the second line is a replacement"
    );
}

/// The archive URL builders, which are real logic — scoped npm names and the
/// sdist-then-wheel preference order — and not worth writing twice.
#[test]
fn the_registry_url_builders_are_reachable_through_the_seam() {
    use diffpack_server::engine::{build_tarball_url, select_pypi_sdist_url, PyPiUrl};

    assert_eq!(
        build_tarball_url("npm", "zod", "4.0.0").unwrap(),
        "https://registry.npmjs.org/zod/-/zod-4.0.0.tgz"
    );
    assert_eq!(
        build_tarball_url("npm", "@types/node", "22.0.0").unwrap(),
        "https://registry.npmjs.org/@types/node/-/node-22.0.0.tgz",
        "a scoped name keeps the scope in the path and drops it from the filename"
    );
    assert!(build_tarball_url("crates", "serde", "1.0.229").is_ok());

    let url = |packagetype: &str, url: &str| PyPiUrl {
        packagetype: packagetype.to_string(),
        url: url.to_string(),
    };
    let chosen = select_pypi_sdist_url(&[
        url(
            "bdist_wheel",
            "https://files.pythonhosted.org/x-py3-none-any.whl",
        ),
        url("sdist", "https://files.pythonhosted.org/x-1.0.tar.gz"),
    ])
    .unwrap();
    assert!(chosen.ends_with(".tar.gz"), "sdist is preferred over wheel");
}
