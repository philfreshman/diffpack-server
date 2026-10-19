use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

pub const SCHEMA: u32 = 1;

const EXPANSION_DIGITS: usize = 25;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DiffKey {
    pub schema: u32,
    pub engine: String,
    pub registry: String,
    pub package: String,
    pub from: String,
    pub to: String,
    pub similarity_threshold: f64,
    pub ignore_whitespace: bool,
}

impl DiffKey {
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

    pub fn diff_id(&self) -> String {
        let digest = Sha256::digest(self.canonical().as_bytes());
        let mut hex = String::with_capacity(64);
        for byte in digest {
            use std::fmt::Write;
            let _ = write!(hex, "{byte:02x}");
        }
        hex
    }

    pub fn meta_path(&self) -> String {
        self.blob_path("meta.json")
    }

    pub fn patches_path(&self) -> String {
        self.blob_path("patches.json")
    }

    fn blob_path(&self, file: &str) -> String {
        format!("{}{}/{}", prefix(self.schema), self.diff_id(), file)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffId(String);

impl DiffId {
    const HEX_DIGITS: usize = 64;

    pub fn parse(text: &str) -> Option<Self> {
        let is_lowercase_hex = |c: char| c.is_ascii_digit() || ('a'..='f').contains(&c);

        (text.len() == Self::HEX_DIGITS && text.chars().all(is_lowercase_hex))
            .then(|| Self(text.to_owned()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn meta_path(&self) -> String {
        self.blob_path("meta.json")
    }

    pub fn patches_path(&self) -> String {
        self.blob_path("patches.json")
    }

    fn blob_path(&self, file: &str) -> String {
        format!("{}{}/{}", prefix(SCHEMA), self.0, file)
    }
}

pub fn prefix(schema: u32) -> String {
    format!("diffs/v{schema}/")
}

fn json_string(value: &str) -> String {
    serde_json::Value::String(value.to_owned()).to_string()
}

fn format_threshold(value: f64) -> String {
    if !value.is_finite() {
        return format!("{value}");
    }

    let expanded = format!("{:.*}", EXPANSION_DIGITS, value.abs());
    let (int_part, frac) = expanded.split_once('.').unwrap_or((&expanded, ""));

    let mut digits: Vec<u8> = frac.bytes().take(4).map(|b| b - b'0').collect();
    digits.resize(4, 0);

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
