//! The deterministic cache key for a diff result.
//!
//! [`docs/cache-key.md`](../docs/cache-key.md) is the normative specification
//! and `fixtures/cache-key-vectors.json` is what both this crate and the
//! TypeScript reader (#27) are tested against. This module implements that
//! document; when the two disagree, the document wins.
//!
//! Read the document before changing anything here. In particular
//! [`DiffKey::canonical`] builds its JSON by concatenation rather than by
//! serialising a struct: the field set is closed, the order is fixed, and a
//! hand-built string cannot be quietly reordered by a `serde` upgrade.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// The payload shape. Bumping it invalidates every cached entry at once, and
/// moves the blob prefix from `diffs/v1/` to `diffs/v2/`.
pub const SCHEMA: u32 = 1;

/// Digits of the exact decimal expansion to take before rounding the
/// threshold to four places.
///
/// The expansion has to be long enough that a value merely *close* to a
/// four-decimal midpoint is distinguishable from one sitting on it. Doubles
/// near `0.1` are spaced about `1.4e-17` apart, so any deviation from a
/// midpoint shows up by the eighteenth decimal place; 25 leaves room without
/// pretending to more.
const EXPANSION_DIGITS: usize = 25;

/// Everything that decides whether two diffs are the same diff.
///
/// `Deserialize` is not decoration either — the test vectors are read into
/// this type, so the field names here and the field names in the fixture are
/// the same names by construction.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DiffKey {
    /// The payload shape. See [`SCHEMA`].
    pub schema: u32,
    /// The `diffpack-engine` version that computed the result: rename
    /// detection and line counts are its behaviour, not ours.
    pub engine: String,
    /// One of `npm`, `crates`, `pypi`.
    pub registry: String,
    /// The package name, verbatim. No case folding, no PEP 503.
    pub package: String,
    /// The base version, verbatim. No `v`-stripping.
    pub from: String,
    /// The target version. Ordered: A→B is not B→A.
    pub to: String,
    /// Rename-detection threshold. A float here, a fixed-precision string in
    /// the key — see [`format_threshold`].
    pub similarity_threshold: f64,
    /// Changes statuses and line counts, so it changes the key.
    pub ignore_whitespace: bool,
}

impl DiffKey {
    /// The canonical JSON string that gets hashed: compact, UTF-8, keys in
    /// ASCII sort order.
    pub fn canonical(&self) -> String {
        format!(
            concat!(
                r#"{{"engine":{},"from":{},"ignore_whitespace":{},"package":{},"#,
                r#""registry":{},"schema":{},"similarity_threshold":{},"to":{}}}"#
            ),
            json_string(&self.engine),
            json_string(&self.from),
            self.ignore_whitespace,
            json_string(&self.package),
            json_string(&self.registry),
            self.schema,
            json_string(&format_threshold(self.similarity_threshold)),
            json_string(&self.to),
        )
    }

    /// `sha256(canonical)` as 64 lowercase hex characters.
    pub fn diff_id(&self) -> String {
        let digest = Sha256::digest(self.canonical().as_bytes());
        let mut hex = String::with_capacity(64);
        for byte in digest {
            use std::fmt::Write;
            let _ = write!(hex, "{byte:02x}");
        }
        hex
    }

    /// Where the result's metadata lives in the blob store.
    pub fn meta_path(&self) -> String {
        self.blob_path("meta.json")
    }

    /// Where the result's patches live in the blob store.
    pub fn patches_path(&self) -> String {
        self.blob_path("patches.json")
    }

    fn blob_path(&self, file: &str) -> String {
        format!("diffs/v{}/{}/{}", self.schema, self.diff_id(), file)
    }
}

/// A JSON string literal, escaping included. `serde_json` owns the escaping
/// rules so that a package name with a quote or a control character in it
/// cannot produce a string the TypeScript side parses differently.
fn json_string(value: &str) -> String {
    serde_json::Value::String(value.to_owned()).to_string()
}

/// The threshold as exactly four decimal places, rounding half away from zero.
///
/// Not `format!("{value:.4}")`, which rounds half to *even* — the two differ
/// for a value whose expansion is exactly a half at the fifth place, and the
/// specification picks half-away-from-zero because that is what TypeScript's
/// `toFixed(4)` does. Rounding the exact decimal expansion by hand is how the
/// two languages are made to agree on the case rather than nearly agree.
fn format_threshold(value: f64) -> String {
    if !value.is_finite() {
        // Nothing sensible to canonicalise, and a panic here would take a
        // whole request down. Round-trips through the same string every time,
        // which is all the key needs.
        return format!("{value}");
    }

    let expanded = format!("{:.*}", EXPANSION_DIGITS, value.abs());
    let (int_part, frac) = expanded.split_once('.').unwrap_or((&expanded, ""));

    let mut digits: Vec<u8> = frac.bytes().take(4).map(|b| b - b'0').collect();
    digits.resize(4, 0);

    // Half away from zero: the fifth digit alone decides, because anything
    // after it can only push the value further from the midpoint.
    let mut carry = frac.as_bytes().get(4).is_some_and(|b| *b >= b'5');
    for digit in digits.iter_mut().rev() {
        if !carry {
            break;
        }
        if *digit == 9 {
            *digit = 0;
        } else {
            *digit += 1;
            carry = false;
        }
    }

    let mut whole: u128 = int_part
        .bytes()
        .fold(0, |n, b| n * 10 + u128::from(b - b'0'));
    if carry {
        whole += 1;
    }

    let sign = if value.is_sign_negative() { "-" } else { "" };
    let frac: String = digits.iter().map(|d| char::from(b'0' + d)).collect();
    format!("{sign}{whole}.{frac}")
}
