use diffpack_server::cache_key::{prefix, DiffKey};
use serde::Deserialize;

#[derive(Deserialize)]
struct Vectors {
    vectors: Vec<Vector>,
}

#[derive(Deserialize)]
struct Vector {
    name: String,
    input: DiffKey,
    canonical: String,
    diff_id: String,
}

fn vectors() -> Vec<Vector> {
    let raw = include_str!("../fixtures/cache-key-vectors.json");
    serde_json::from_str::<Vectors>(raw)
        .expect("fixtures/cache-key-vectors.json should parse")
        .vectors
}

#[test]
fn every_vector_produces_its_canonical_string() {
    for v in vectors() {
        assert_eq!(v.input.canonical(), v.canonical, "canonical for {}", v.name);
    }
}

#[test]
fn every_vector_produces_its_diff_id() {
    for v in vectors() {
        assert_eq!(v.input.diff_id(), v.diff_id, "diff_id for {}", v.name);
    }
}

#[test]
fn a_diff_id_is_sixty_four_lowercase_hex_characters() {
    for v in vectors() {
        let id = v.input.diff_id();
        assert_eq!(id.len(), 64, "length for {}", v.name);
        assert!(
            id.bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)),
            "{} produced {id}, which is not lowercase hex",
            v.name
        );
    }
}

#[test]
fn the_fixture_covers_the_cases_the_specification_calls_out() {
    let names: Vec<String> = vectors().into_iter().map(|v| v.name).collect();
    for expected in [
        "npm/zod",
        "npm/@types/node scoped name",
        "pypi/typing_extensions underscore",
        "pypi/Typing.Extensions mixed separators and case",
        "crates/serde",
        "threshold 0.75 formats as 0.7500",
        "ignore_whitespace true",
        "order A to B",
        "order B to A",
    ] {
        assert!(
            names.iter().any(|n| n == expected),
            "missing vector: {expected}"
        );
    }
}

#[test]
fn reversing_the_version_pair_changes_the_key() {
    let forward = find("order A to B");
    let backward = find("order B to A");

    assert_eq!(forward.input.package, backward.input.package);
    assert_ne!(forward.input.diff_id(), backward.input.diff_id());
}

#[test]
fn pypi_names_are_not_normalised() {
    let ids: std::collections::HashSet<String> = [
        "pypi/typing_extensions underscore",
        "pypi/typing-extensions hyphen",
        "pypi/Typing.Extensions mixed separators and case",
    ]
    .iter()
    .map(|n| find(n).input.diff_id())
    .collect();

    assert_eq!(
        ids.len(),
        3,
        "the three spellings collapsed onto each other"
    );
}

fn find(name: &str) -> Vector {
    vectors()
        .into_iter()
        .find(|v| v.name == name)
        .unwrap_or_else(|| panic!("no vector named {name}"))
}

#[test]
fn the_key_changes_when_and_only_when_a_field_changes() {
    let base = find("npm/zod").input;
    let id = base.diff_id();

    let mutations: Vec<(&str, DiffKey)> = vec![
        (
            "schema",
            DiffKey {
                schema: base.schema + 1,
                ..base.clone()
            },
        ),
        (
            "engine",
            DiffKey {
                engine: "0.4.0".into(),
                ..base.clone()
            },
        ),
        (
            "registry",
            DiffKey {
                registry: "crates".into(),
                ..base.clone()
            },
        ),
        (
            "package",
            DiffKey {
                package: "zod-core".into(),
                ..base.clone()
            },
        ),
        (
            "from",
            DiffKey {
                from: "3.25.77".into(),
                ..base.clone()
            },
        ),
        (
            "to",
            DiffKey {
                to: "4.0.1".into(),
                ..base.clone()
            },
        ),
        (
            "similarity_threshold",
            DiffKey {
                similarity_threshold: 0.6,
                ..base.clone()
            },
        ),
        (
            "ignore_whitespace",
            DiffKey {
                ignore_whitespace: !base.ignore_whitespace,
                ..base.clone()
            },
        ),
    ];

    for (field, mutated) in &mutations {
        assert_ne!(mutated.diff_id(), id, "changing {field} left the key alone");
    }

    let ids: std::collections::HashSet<String> =
        mutations.iter().map(|(_, k)| k.diff_id()).collect();
    assert_eq!(
        ids.len(),
        mutations.len(),
        "two single-field changes collided"
    );

    assert_eq!(
        base.clone().diff_id(),
        id,
        "the key moved without a field moving"
    );
}

#[test]
fn thresholds_agreeing_to_four_places_are_one_entry_and_no_more_than_that() {
    let coarse = find("npm/zod").input;
    assert_eq!(coarse.similarity_threshold, 0.75);

    let indistinguishable = find("threshold 0.750000001 collapses onto 0.7500").input;
    assert_eq!(
        indistinguishable.diff_id(),
        coarse.diff_id(),
        "0.750000001 should be the same cache entry as 0.75"
    );

    let distinguishable = find("threshold 0.7501 is a different key").input;
    assert_ne!(
        distinguishable.diff_id(),
        coarse.diff_id(),
        "0.7501 differs at the fourth place and should be a separate entry"
    );
}

#[test]
fn the_threshold_is_always_four_decimal_places_in_the_canonical_string() {
    for (name, expected) in [
        ("threshold 0.75 formats as 0.7500", "0.7500"),
        ("threshold 0.8 formats as 0.8000", "0.8000"),
        ("threshold 1 formats as 1.0000", "1.0000"),
        ("threshold 0.12345 rounds to 0.1235", "0.1235"),
    ] {
        let canonical = find(name).input.canonical();
        assert!(
            canonical.contains(&format!(r#""similarity_threshold":"{expected}""#)),
            "{name}: expected {expected} in {canonical}"
        );
    }
}

#[test]
fn blob_paths_are_the_diff_id_under_the_schema_prefix() {
    let key = find("npm/zod").input;
    let id = key.diff_id();

    assert_eq!(key.meta_path(), format!("diffs/v1/{id}/meta.json"));
    assert_eq!(key.patches_path(), format!("diffs/v1/{id}/patches.json"));

    for path in [key.meta_path(), key.patches_path()] {
        assert!(
            path.starts_with(&prefix(key.schema)),
            "`{path}` should be under `{}`",
            prefix(key.schema)
        );
    }
}

#[test]
fn a_go_key_is_a_new_registry_value_in_the_same_schema() {
    let key = DiffKey {
        schema: 1,
        engine: "0.4.1".into(),
        registry: "go".into(),
        package: "github.com/Masterminds/semver/v3".into(),
        from: "v3.2.0".into(),
        to: "v3.2.1".into(),
        similarity_threshold: 0.75,
        ignore_whitespace: false,
    };

    assert_eq!(
        key.canonical(),
        r#"{"engine":"0.4.1","from":"v3.2.0","ignore_whitespace":false,"package":"github.com/Masterminds/semver/v3","registry":"go","schema":1,"similarity_threshold":"0.7500","to":"v3.2.1"}"#,
        "the module path goes in verbatim, capitals and `/v3` included, and the \
         schema stays 1 because a registry was always a field of the key"
    );
    assert_eq!(
        key.diff_id(),
        "ba57f2372213c64cb75657d80ce12cabca61fadc8897d03b42aa31bd1b22a5dd",
        "sha256 of the canonical string above, computed outside this crate"
    );
}
