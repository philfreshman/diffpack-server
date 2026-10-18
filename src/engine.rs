use crate::archive::FileMap;

pub use diffpack_engine::{
    archive_source, build_patch, choose_archive, get_diff_content, unpack_archive, whitespace_mode,
    ArchiveSource, DiffFileEntry, DiffStatus, FileMapEntry, FileType, Patch, WhitespaceMode,
};

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

pub const VERSION: &str = "0.4.1";
