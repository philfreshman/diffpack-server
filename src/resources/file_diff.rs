//! `diffpack://diff/{handle}/file/{path}` — one file's diff out of a
//! comparison.
//!
//! The same bytes [`get_file_diff`](crate::tools::get_file_diff) returns at
//! its defaults, which is three lines of unchanged context around each change
//! — the settings a caller gets by passing nothing but a handle and a path,
//! which is all a URI has room for. The tool is *called* rather than
//! reproduced, so the two cannot render one file two ways.
//!
//! That includes not rendering it at all. What answers is
//! [`diff_package_versions::Comparison::file_patch`], the one question both
//! of them ask: the patch a remembered comparison already holds, a directory
//! refused out of the tree, and both archives only for a file with neither.
//! A read that downloaded where a call did not would be the two disagreeing
//! about what the cache is for, and asking one function is what makes that a
//! thing that cannot happen rather than two call sites kept in step.
//!
//! # What the media type carries
//!
//! What the tool says in `isDiff`, this says in the one field a client
//! already reads to decide how to render something: a patch is `text/x-diff`
//! and the two answers that are not patches — a file both versions ship byte
//! for byte, and a path neither version has — are `text/plain`. A reader that
//! had to check a field first would eventually parse `@@` out of a file that
//! has none.
//!
//! # Why the path is decoded
//!
//! `{path}` is simple expansion under RFC 6570, so a client that follows the
//! spec percent-encodes the reserved characters — including the `/` that
//! nearly every file path has. [`decoded`] resolves those escapes before the
//! lookup, and an escape that is not one is a refusal rather than a miss.
//!
//! # Why the old path is looked up rather than asked for
//!
//! A renamed file needs both of its paths, and the tool's own description
//! tells a caller to pass the `old_path` the tree gives it. A URI has room
//! for one path, so this looks the other up in the comparison rather than
//! leaving it out — the alternative being an answer that reports every line
//! of a moved file as added, which is wrong about a file the package still
//! ships and which a reader has nothing in the answer to doubt with. It is
//! the same departure `get_file_diff` makes for a directory, and for the same
//! reason.

use std::borrow::Cow;

use rmcp::model::{CacheScope, ReadResourceResult, ResourceContents, ResourceTemplate};

use crate::engine::DiffFileEntry;
use crate::error::Failure;
use crate::handle::DiffHandle;
use crate::tools::get_file_diff::{self, OneFile};
use crate::tools::{diff_package_versions, get_diff_tree, Ctx};

/// What stands between the handle and the path.
const SEPARATOR: &str = "/file/";

/// The URI a client fills in to read one file's diff.
pub fn uri_template() -> String {
    format!("{}{{handle}}{SEPARATOR}{{path}}", super::diff::PREFIX)
}

/// The handle and the path `uri` names, if this is a URI of ours at all.
///
/// The path is taken to the end of the URI, separators and all: it is the
/// last thing there, so there is nothing after it to be confused with. Its
/// escapes are still in it — [`decoded`] resolves those, and doing it here
/// would mean matching on a string this function had already changed. A
/// handle carries no `/`, which is what tells this apart from the whole
/// comparison's URI without either matcher having to be tried first.
pub fn parts_in(uri: &str) -> Option<(&str, &str)> {
    let rest = uri.strip_prefix(super::diff::PREFIX)?;
    let (handle, path) = rest.split_once(SEPARATOR)?;

    (!handle.is_empty() && !handle.contains('/') && !path.is_empty()).then_some((handle, path))
}

/// The template, as `resources/templates/list` shows it.
pub fn template() -> ResourceTemplate {
    ResourceTemplate::new(uri_template(), "file-diff")
        .with_title("File diff")
        .with_description(
            "One file's diff out of a comparison, with three lines of unchanged context \
             around each change. `{handle}` is the handle `diff_package_versions` gave \
             you and `{path}` is a path from that comparison's tree, with the archive's \
             top-level directory already removed. A `text/x-diff` answer is a patch; a \
             `text/plain` one is not — a file both versions ship unchanged comes back as \
             itself, and a path neither version has comes back as a sentence saying so.",
        )
        .with_mime_type("text/x-diff")
}

/// How long a client may treat one file's diff as fresh.
///
/// A day, for the reason the whole comparison gets one: the handle names two
/// published versions and the build that compared them, so this answer cannot
/// change without the handle being refused outright.
const TTL_MS: u64 = 24 * 60 * 60 * 1000;

/// One file of one comparison.
pub async fn read(
    handle: &DiffHandle,
    path: &str,
    ctx: &Ctx,
) -> Result<ReadResourceResult, Failure> {
    // Before the comparison rather than after it: a URI the client cannot
    // have meant is refused without two archives being downloaded to find
    // out.
    let wanted = decoded(path)?;

    let comparison = diff_package_versions::compare(handle, ctx).await?;

    let moved = moved_from(&comparison.tree, &wanted);
    let asked = OneFile {
        path: &wanted,
        old_path: moved.as_deref(),
        // The defaults a caller gets by passing nothing but a handle and a
        // path, which is all a URI has room for.
        context_lines: get_file_diff::ContextLines::default(),
        max_bytes: None,
    };

    // The tool's own answer, asked of the comparison the way the tool asks
    // it, because this document is that answer with a media type on it (ADR
    // 0014). Whether it was stored, and what it costs when it was not, is the
    // comparison's to decide rather than this module's.
    let patch = comparison.file_patch(ctx, asked).await?;

    // The segment as it arrived rather than as it decoded, so a client that
    // encoded its path is answered at the URI it asked about. The two are
    // the same string for a path that had nothing to escape.
    let contents = ResourceContents::text(
        patch.excerpt.text,
        format!("{}{SEPARATOR}{path}", super::diff::uri_of(handle)),
    )
    .with_mime_type(if patch.is_diff {
        "text/x-diff"
    } else {
        "text/plain"
    });

    Ok(ReadResourceResult::new(vec![contents])
        .with_ttl_ms(TTL_MS)
        .with_cache_scope(CacheScope::Public))
}

/// The `{path}` segment, with its percent-escapes resolved.
///
/// `{path}` is simple expansion under RFC 6570, which percent-encodes the
/// reserved characters — `/` among them. Very nearly every value this
/// template is for has a `/` in it, so a client that expands the template the
/// way the spec says asks for `src%2Findex.js`, and a lookup of that string
/// finds nothing: the resource would have told a conforming client that every
/// file it asked about was in neither version.
///
/// Only this segment. The handle before it is decoded by
/// [`DiffHandle::decode`] out of an alphabet with nothing to escape, and it
/// is matched before this runs — so there is one segment here whose spelling
/// is the caller's, and it is the only one taken apart this way.
///
/// The decoded value is a key in a [`FileMap`](crate::archive::FileMap) and
/// never a path on a disk, which is what makes `%2F` ordinary rather than
/// something to defend against: `..%2F..%2Fetc%2Fpasswd` becomes a key no
/// comparison has, and gets the sentence any other absent path gets.
///
/// Written here on `std` rather than taken from a crate because a resource
/// module may import the standard library, `futures` and the protocol crates
/// and nothing else — `scripts/check-tool-seams.sh` holds it to that — and
/// twenty lines is a smaller thing to answer for than a hole in that list.
fn decoded(segment: &str) -> Result<Cow<'_, str>, Failure> {
    // The common case and the one that must not allocate: a path with
    // nothing to escape is already what it decodes to.
    if !segment.contains('%') {
        return Ok(Cow::Borrowed(segment));
    }

    let raw = segment.as_bytes();
    let mut bytes = Vec::with_capacity(raw.len());
    let mut at = 0;

    while at < raw.len() {
        let byte = raw[at];

        if byte != b'%' {
            bytes.push(byte);
            at += 1;
            continue;
        }

        // Two digits, both hexadecimal. `to_digit` and not `from_str_radix`,
        // which accepts a leading sign and would read `%+f` as an escape.
        let (Some(high), Some(low)) = (
            raw.get(at + 1).and_then(|&digit| hex(digit)),
            raw.get(at + 2).and_then(|&digit| hex(digit)),
        ) else {
            return Err(malformed(segment));
        };

        bytes.push(high * 16 + low);
        at += 3;
    }

    // A path is text here — a `FileMap` is keyed by one — so bytes that spell
    // no string are malformed rather than something to render lossily. That
    // is the opposite of what extraction does to a file's *content*, and for
    // the opposite reason: there the bytes are the answer, and here they are
    // what names it.
    String::from_utf8(bytes)
        .map(Cow::Owned)
        .map_err(|_| malformed(segment))
}

/// One hexadecimal digit's value, or nothing if it is not one.
fn hex(digit: u8) -> Option<u8> {
    char::from(digit)
        .to_digit(16)
        .and_then(|value| u8::try_from(value).ok())
}

/// A path whose escapes decode to nothing, as a refusal.
///
/// [`Failure::InvalidParams`] and not [`Failure::NoSuchResource`]: the URI is
/// one of ours and the client's own escape is what is wrong with it. Both are
/// `-32602`, but a read that fell through to the lookup instead would answer
/// a broken URI with "File not present in either version." — the sentence a
/// path the package really does not ship gets, which is a client told its
/// typo is a fact about the package.
fn malformed(segment: &str) -> Failure {
    Failure::InvalidParams {
        message: format!(
            "`{segment}` is not a path this server can read: a `%` in a URI introduces \
             two hexadecimal digits standing for one byte, and these spell no text. Pass \
             the path as the comparison's tree gives it, percent-encoded or not."
        ),
    }
}

/// Where the file at `path` was in the first version, if it moved.
///
/// The comparison's own answer, which is the one the tool's description tells
/// a caller to pass. Nothing for every other file, which is what the tool
/// wants for them — and nothing for a path the comparison does not have,
/// which the renderer answers with the sentence saying so.
///
/// A descent rather than a walk, which is `get_diff_tree`'s and not this
/// module's: one step per directory rather than a scan of everything above
/// the file.
fn moved_from(tree: &DiffFileEntry, path: &str) -> Option<String> {
    get_diff_tree::node_at(tree, path)?.old_path.clone()
}
