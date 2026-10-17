use serde::{Deserialize, Serialize};

use crate::archive::FileMap;

pub use diffpack_engine::{
    build_tarball_url, extract_archive_bytes, get_diff_content, select_pypi_sdist_url,
    whitespace_mode, DiffFileEntry, DiffStatus, FileMapEntry, FileType, PyPiResponse, PyPiUrl,
    WhitespaceMode,
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Patch {
    pub data: String,
    pub is_diff: bool,
}

pub fn patch(
    filename: &str,
    from: Option<&str>,
    to: Option<&str>,
    ignore_whitespace: bool,
) -> Patch {
    match (from, to) {
        (None, None) => Patch {
            data: "File not present in either version.".to_owned(),
            is_diff: false,
        },

        (None, Some(to)) => Patch {
            data: sided(&format!("--- /dev/null\n+++ to/{filename}"), '+', to),
            is_diff: true,
        },

        (Some(from), None) => Patch {
            data: sided(&format!("--- from/{filename}\n+++ /dev/null"), '-', from),
            is_diff: true,
        },

        (Some(from), Some(to)) if from == to => Patch {
            data: to.to_owned(),
            is_diff: false,
        },

        (Some(from), Some(to)) => Patch {
            data: get_diff_content(filename, from, to, ignore_whitespace),
            is_diff: true,
        },
    }
}

pub fn build_diff_tree(
    from: &FileMap,
    to: &FileMap,
    similarity_threshold: f64,
    ignore_whitespace: bool,
) -> DiffFileEntry {
    diffpack_engine::build_diff_tree(
        from.as_engine_map(),
        to.as_engine_map(),
        similarity_threshold,
        ignore_whitespace,
    )
}

fn sided(header: &str, sign: char, content: &str) -> String {
    let mut rendered = String::from(header);
    for line in content.split('\n') {
        rendered.push('\n');
        rendered.push(sign);
        rendered.push(' ');
        rendered.push_str(line);
    }
    rendered
}

pub const VERSION: &str = "0.3.0";
