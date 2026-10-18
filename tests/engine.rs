use diffpack_server::engine;

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

#[test]
fn the_tree_builder_is_reachable_through_the_seam() {
    use diffpack_server::archive::FileMap;
    use diffpack_server::engine::{build_diff_tree, DiffStatus, FileMapEntry, FileType};
    use std::collections::HashMap;

    let file = |content: &str| FileMapEntry {
        file_type: FileType::File,
        content: content.to_string(),
    };

    let from = FileMap::from(HashMap::from([("a.txt".to_string(), file("one\ntwo\n"))]));
    let to = FileMap::from(HashMap::from([("a.txt".to_string(), file("one\nTWO\n"))]));

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

#[test]
fn the_archive_extractor_is_reachable_through_the_seam() {
    let result = diffpack_server::engine::unpack_archive("npm", "zod", "4.0.0", b"not an archive");
    assert!(result.is_err(), "rubbish bytes should not extract");
}

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

#[test]
fn the_registry_archive_lookup_is_reachable_through_the_seam() {
    use diffpack_server::engine::{archive_source, choose_archive, ArchiveSource};

    assert_eq!(
        archive_source("npm", "zod", "4.0.0").unwrap(),
        ArchiveSource::Archive {
            url: "https://registry.npmjs.org/zod/-/zod-4.0.0.tgz".to_string()
        }
    );
    assert_eq!(
        archive_source("npm", "@types/node", "22.0.0").unwrap(),
        ArchiveSource::Archive {
            url: "https://registry.npmjs.org/@types/node/-/node-22.0.0.tgz".to_string()
        },
        "a scoped name keeps the scope in the path and drops it from the filename"
    );
    assert!(archive_source("crates", "serde", "1.0.229").is_ok());
    assert_eq!(
        archive_source("pypi", "requests", "2.31.0").unwrap(),
        ArchiveSource::Listing {
            url: "https://pypi.org/pypi/requests/2.31.0/json".to_string()
        },
        "PyPI's archive is named in its metadata, so that is what is fetched first"
    );

    let chosen = choose_archive(
        "pypi",
        r#"{"urls":[
            {"packagetype":"bdist_wheel","url":"https://files.pythonhosted.org/x-py3-none-any.whl"},
            {"packagetype":"sdist","url":"https://files.pythonhosted.org/x-1.0.tar.gz"}
        ]}"#,
    )
    .unwrap();
    assert!(chosen.ends_with(".tar.gz"), "sdist is preferred over wheel");
}
