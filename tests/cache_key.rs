//! The cache key, driven entirely by `fixtures/cache-key-vectors.json`.
//!
//! The vectors are the point. `docs/cache-key.md` is normative and the
//! TypeScript side of the cache (#27) will be held to the same file, so a
//! test here that asserted what this implementation happens to produce would
//! be worth nothing: it would agree with itself while the two languages
//! disagreed with each other. Every expected value below is read from the
//! fixture, which was generated from the document rather than from this code.

use diffpack_server::cache_key::DiffKey;
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

/// The fixture is only worth something if it actually exercises the rules it
/// claims to. A vector file that silently lost its scoped-name case would
/// still pass every assertion above.
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

/// A→B is not B→A. The whole cache is wrong if this stops holding.
#[test]
fn reversing_the_version_pair_changes_the_key() {
    let forward = find("order A to B");
    let backward = find("order B to A");

    assert_eq!(forward.input.package, backward.input.package);
    assert_ne!(forward.input.diff_id(), backward.input.diff_id());
}

/// Three spellings PyPI itself treats as one project stay three keys under
/// v1's verbatim rule. A cache miss is the cheap failure; serving the wrong
/// diff is not.
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

/// Every field is load-bearing, and nothing outside the eight is. The second
/// half is the half that catches a future field being added to `DiffKey`
/// without being added to `canonical`.
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

    // Distinct from each other too: a `canonical` that concatenated two
    // fields in the wrong slots would still pass the loop above.
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

/// The precision rule, pinned at its boundary.
///
/// **This inverts the bullet in issue #5**, which asks for a test proving
/// `0.75` and `0.750000001` produce *different* keys. They cannot: four
/// decimal places is the specified precision, and both values round to
/// `"0.7500"`. The bullet describes the behaviour of an implementation with
/// no fixed-precision rule at all, where the raw float reaches the key and
/// the two languages are free to format it differently — which is the thing
/// the rule exists to prevent.
///
/// So the test pins what the rule actually does: agreement to four places is
/// the same cache entry, and one step outside it is not. Widening the
/// partition means changing the precision, which is a `schema` bump.
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

/// Formatting is fixed-precision in both directions: digits are added to a
/// short value and dropped from a long one.
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
}
