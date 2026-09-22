//! The resources, driven the way a client drives them.
//!
//! One seam: the wire. `resources/list`, `resources/templates/list` and
//! `resources/read` through `router_with` over the fixture archives, which is
//! how `tests/get_diff_tree.rs` drives the tools. A resource is a URI and
//! nothing else — there is no schema a client reads first — so the URI
//! reaching the code that answers it is most of what can break, and a handler
//! test would pass through a URI that never matched.

mod common;

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine as _;
use common::{Client, PREVIOUS};
use diffpack_server::handle::{DiffHandle, Inputs};
use diffpack_server::page;
use diffpack_server::registry::{ArchiveSource, Registry, VERSION_RULE};
use serde_json::{json, Value};

/// The registry catalogue's URI.
const REGISTRIES: &str = "diffpack://registries";

/// A whole comparison, by the handle that names it.
const DIFF_TEMPLATE: &str = "diffpack://diff/{handle}";

/// One file's diff out of that comparison.
const FILE_TEMPLATE: &str = "diffpack://diff/{handle}/file/{path}";

// ---------------------------------------------------------------------------
// What a client is told there is
// ---------------------------------------------------------------------------

/// A client is told there are resources before it asks for any.
///
/// `server/discover` replaced the handshake, so the capabilities there are
/// the only thing that tells a client `resources/list` is worth sending. The
/// list answers either way, which is exactly why this is its own test: a
/// server that served resources and advertised none would pass every other
/// test in this file and reach a client as a server that has none.
#[tokio::test]
async fn a_client_is_told_this_server_has_resources() {
    let answer = current(json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "server/discover",
        "params": {},
    }))
    .await;

    assert!(
        answer["result"]["capabilities"]["resources"].is_object(),
        "a server offering resources should say so where a client looks, got {}",
        answer["result"]["capabilities"]
    );
}

/// A client on the previous revision finds them and reads one too.
///
/// One endpoint serves both revisions — `tests/mcp.rs` says why — and a
/// client on `2025-11-25` sends none of what the current one does: no
/// per-request `_meta`, no `Mcp-Method`, no `Mcp-Name`. Worth its own test
/// because everything else here is written as a `2026-07-28` client, so a
/// resource that only answered one of the two would pass the whole file.
#[tokio::test]
async fn a_previous_revision_client_lists_and_reads_a_resource() {
    let listed = previous(json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "resources/list",
        "params": {},
    }))
    .await;

    assert_eq!(
        listed["result"]["resources"][0]["uri"],
        json!(REGISTRIES),
        "got {listed}"
    );

    let read = previous(json!({
        "jsonrpc": "2.0",
        "id": 2,
        "method": "resources/read",
        "params": { "uri": REGISTRIES },
    }))
    .await;

    assert!(
        read["result"]["contents"][0]["text"].is_string(),
        "a client on the previous revision should read a resource too, got {read}"
    );
}

/// The catalogue is a resource a client can find without being told.
///
/// `resources/list` is where a client looks, and a server that answers it
/// with nothing is one whose resources exist only for a caller that already
/// knew the URI.
#[tokio::test]
async fn the_registry_catalogue_is_listed() {
    let listed = resources().await;

    let catalogue = listed
        .iter()
        .find(|resource| resource["uri"] == REGISTRIES)
        .unwrap_or_else(|| panic!("`{REGISTRIES}` should be listed, got {listed:?}"));

    assert!(
        catalogue["name"].is_string(),
        "a listed resource carries the name a client shows, got {catalogue}"
    );
}

/// The two diffs are templates, which is a different method.
///
/// `resources/list` carries `Resource`s, which have a `uri` a client can read
/// as it stands; a URI with a `{handle}` in it is not one of those and
/// `resources/templates/list` is where the `2026-07-28` schema puts it. A
/// template listed as a resource would be a URI a client followed literally
/// and got `-32602` for.
#[tokio::test]
async fn both_diff_templates_are_listed() {
    let listed = templates().await;
    let uris: Vec<&str> = listed
        .iter()
        .filter_map(|template| template["uriTemplate"].as_str())
        .collect();

    assert_eq!(
        uris,
        vec![DIFF_TEMPLATE, FILE_TEMPLATE],
        "both diffs should be listed as templates, got {listed:?}"
    );
}

/// And the catalogue is not among them.
///
/// The other half of the split: a URI a client can read as it stands belongs
/// in `resources/list`, and listing it twice would have a client read it
/// twice.
#[tokio::test]
async fn the_catalogue_is_not_a_template() {
    let listed = templates().await;

    assert!(
        !listed
            .iter()
            .any(|template| template["uriTemplate"] == REGISTRIES),
        "`{REGISTRIES}` has nothing to fill in, got {listed:?}"
    );
}

/// Every description a client is shown reads as a sentence.
///
/// These are the prose a model picks a resource on, and they are written as
/// Rust string literals continued across lines with a trailing `\`. Drop one
/// continuation and the literal keeps the source's indentation instead: the
/// sentence still compiles, still says the right words, and reaches a model
/// with a dozen spaces in the middle of it. Nothing else here would notice,
/// because every other assertion in this file is about a URI or a field.
///
/// Over the whole of what a client is told rather than one description: the
/// one that loses its continuation next is in a resource nobody has written
/// yet.
#[tokio::test]
async fn no_description_a_client_reads_has_lost_a_line_continuation() {
    let mut ragged = Vec::new();

    for (where_from, description) in advertised().await {
        if description.contains("  ") {
            ragged.push(format!("{where_from}: {description}"));
        }
    }

    assert!(
        ragged.is_empty(),
        "these reach a model with the source's indentation in them: {ragged:#?}"
    );
}

// ---------------------------------------------------------------------------
// The registry catalogue
// ---------------------------------------------------------------------------

/// The catalogue answers with all three registries and where each serves
/// from.
///
/// The URLs are written out rather than derived from the module, which is the
/// opposite of what the test below does and is the point of having both: this
/// one is a known-good literal — the address npm publishes its tarballs at —
/// so a projection that faithfully reproduced a wrong module would fail here.
#[tokio::test]
async fn the_catalogue_names_every_registry_and_where_its_archives_are() {
    let catalogue = read(REGISTRIES).await;
    let described = registries(&catalogue);

    let ids: Vec<&str> = described
        .iter()
        .filter_map(|registry| registry["id"].as_str())
        .collect();
    assert_eq!(
        ids,
        vec!["npm", "crates", "pypi"],
        "all three registries should be described, got {catalogue}"
    );

    let patterns: Vec<&str> = described
        .iter()
        .filter_map(|registry| registry["archive"]["url_pattern"].as_str())
        .collect();
    assert_eq!(
        patterns,
        vec![
            "https://registry.npmjs.org/{package}/-/{package-without-scope}-{version}.tgz",
            "https://static.crates.io/crates/{package}/{package}-{version}.crate",
            "https://pypi.org/pypi/{package}/{version}/json",
        ],
        "each registry should say where a version's archive comes from, got {catalogue}"
    );
}

/// npm's archive path carries a scoped name twice and differently.
///
/// The whole name is the path segment and the name without its scope is in
/// the filename, so `@types/node` is served from `.../@types/node/-/node-…`.
/// A pattern that wrote `{package}` in both slots would be right about every
/// unscoped package and wrong about the one kind of name this resource exists
/// to stop an agent guessing at.
#[tokio::test]
async fn npms_pattern_distinguishes_a_scoped_name_from_the_name_alone() {
    let catalogue = read(REGISTRIES).await;
    let npm = registries(&catalogue).remove(0);

    let filled = npm["archive"]["url_pattern"]
        .as_str()
        .expect("an archive pattern is a string")
        .replace("{package-without-scope}", "node")
        .replace("{package}", "@types/node")
        .replace("{version}", "20.1.0");

    assert_eq!(
        filled,
        Registry::Npm
            .archive("@types/node", "20.1.0")
            .expect("npm builds an archive URL")
            .url(),
        "filling npm's pattern in should give the URL the module builds"
    );
}

/// The catalogue is the registry module, not a copy of it.
///
/// The criterion #16 states, and the reason #42 put it there: this resource
/// restates what four other places know separately, so the only thing that
/// keeps the five in step is that the resource is *derived*. Every field is
/// checked against what `crate::registry` answers, and the field list is
/// checked too — a field added to the document without an agreement of its
/// own fails here rather than going unasserted until it is wrong.
///
/// The patterns are checked by filling them in: `{package}` and the rest are
/// replaced by the values the module is then asked about, so the derivation
/// is inverted rather than repeated. Every value is spelled in unreserved
/// characters, which is what makes a filled pattern comparable with a URL the
/// module escaped.
#[tokio::test]
async fn the_catalogue_is_the_registry_module_rather_than_a_copy_of_it() {
    let catalogue = read(REGISTRIES).await;

    assert_eq!(
        fields(&catalogue),
        vec!["registries", "version_rule"],
        "a field added to the catalogue needs an agreement of its own, got {catalogue}"
    );
    assert_eq!(
        catalogue["version_rule"], VERSION_RULE,
        "the version rule should be the module's, got {catalogue}"
    );

    let described = registries(&catalogue);
    assert_eq!(
        described.len(),
        Registry::ALL.len(),
        "every registry this server has should be described, got {catalogue}"
    );

    for (registry, described) in Registry::ALL.into_iter().zip(described) {
        let id = registry.id();

        assert_eq!(
            fields(&described),
            vec!["archive", "id", "name", "name_rule", "search", "versions"],
            "a field added to `{id}` needs an agreement of its own, got {described}"
        );
        assert_eq!(described["id"], id, "got {described}");
        assert_eq!(described["name"], registry.name(), "got {described}");
        assert_eq!(
            described["name_rule"],
            registry.name_rule(),
            "got {described}"
        );

        // Where a version's archive is, and whether that URL is the archive.
        let archive = registry
            .archive(PACKAGE, VERSION)
            .expect("every registry says where an archive is");
        assert_eq!(
            fields(&described["archive"]),
            vec!["kind", "url_pattern"],
            "got {described}"
        );
        assert_eq!(
            described["archive"]["kind"],
            match archive {
                ArchiveSource::Archive { .. } => "archive",
                ArchiveSource::Listing { .. } => "listing",
            },
            "got {described}"
        );
        assert_eq!(
            filled(&described["archive"]["url_pattern"]),
            archive.url(),
            "filling `{id}`'s archive pattern in should give the URL the module builds"
        );

        // Where a package's versions are listed.
        assert_eq!(
            fields(&described["versions"]),
            vec!["url_pattern"],
            "got {described}"
        );
        assert_eq!(
            filled(&described["versions"]["url_pattern"]),
            registry.versions(PACKAGE).url,
            "filling `{id}`'s versions pattern in should give the URL the module builds"
        );

        // Where a search is answered, and what to ask it for.
        let search = registry.search(QUERY, LIMIT);
        assert_eq!(
            fields(&described["search"]),
            vec!["accept", "url_pattern", "whole_index"],
            "got {described}"
        );
        assert_eq!(
            filled(&described["search"]["url_pattern"]),
            search.url,
            "filling `{id}`'s search pattern in should give the URL the module builds"
        );
        assert_eq!(
            described["search"]["accept"], search.accept,
            "got {described}"
        );
        assert_eq!(
            described["search"]["whole_index"], search.whole_index,
            "got {described}"
        );
    }
}

// ---------------------------------------------------------------------------
// One comparison
// ---------------------------------------------------------------------------

/// Reading a comparison answers what the two tools answer.
///
/// The whole of what this resource is for: the same comparison, reachable by
/// URI instead of by two calls. A reader that got different totals from the
/// resource and the tool would have two answers to one question and nothing
/// to say which was wrong — so both halves are held against the tools that
/// own them, the totals against `diff_package_versions` and the tree against
/// a full walk of `get_diff_tree`.
#[tokio::test]
async fn reading_a_comparison_answers_what_the_tools_answer() {
    let summary = call(
        SUMMARY,
        json!({
            "registry": "npm",
            "package": "diffable",
            "from_version": "1.0.0",
            "to_version": "2.0.0",
        }),
    )
    .await;

    let handle = summary["structuredContent"]["handle"]
        .as_str()
        .unwrap_or_else(|| panic!("the summary carries a handle, got {summary}"))
        .to_owned();

    let document = read(&diff_uri(&handle)).await;

    assert_eq!(
        document["totals"], summary["structuredContent"]["totals"],
        "the resource and the summary should count one comparison the same way, \
         got {document} against {summary}"
    );
    assert_eq!(
        document["tree"],
        json!(walk(&handle).await),
        "the resource's tree should be the tree the paginating tool walks to"
    );
}

/// And it says what was compared.
///
/// A document read out of a client's resource browser has no call beside it
/// saying what was asked for, and the handle in the URI is opaque. Without
/// the inputs the totals are a comparison of something.
#[tokio::test]
async fn reading_a_comparison_says_what_was_compared() {
    let document = read(&diff_uri(&diffable())).await;

    assert_eq!(
        document["inputs"],
        json!({
            "registry": "npm",
            "package": "diffable",
            "from_version": "1.0.0",
            "to_version": "2.0.0",
            "similarity_threshold": 0.75,
            "ignore_whitespace": false,
        }),
        "a comparison should name everything it was made from, got {document}"
    );
}

/// A comparison nothing has computed is read anyway.
///
/// The eviction case, and the only case there is here: each request is served
/// by a server built for it, with no cache behind it and nothing kept between
/// calls, so the handle below names a comparison this process has never made.
/// ADR 0006 is why it can be answered at all — the inputs travel in the URI
/// beside the `diff_id`, so a miss costs a recomputation instead of a URI
/// that has quietly stopped resolving. A `diff_id` in this segment could only
/// ever have been read out of the cache.
#[tokio::test]
async fn a_comparison_nothing_has_computed_is_recomputed_rather_than_refused() {
    let document = read(&diff_uri(&diffable())).await;

    assert_eq!(
        document["totals"]["modified"],
        json!(1),
        "a comparison nobody has made yet is one this server can make: got {document}"
    );
}

/// A URI that is not one of ours resolves to nothing, and says so.
///
/// `-32602`, which is where the `2026-07-28` revision moved resource-not-found
/// from `-32002` — the code it always described. The protocol channel, because
/// there is no resource to have failed and the client, which was told the list,
/// is who can fix the call.
#[tokio::test]
async fn a_uri_this_server_does_not_serve_is_invalid_params() {
    for uri in [
        "diffpack://nothing-here",
        "https://example.invalid/diff",
        // The templates as they are listed, followed literally.
        DIFF_TEMPLATE,
        FILE_TEMPLATE,
    ] {
        let answer = reading(uri).await;

        assert_eq!(
            answer["error"]["code"], -32602,
            "`{uri}` resolves to nothing: got {answer}"
        );
    }
}

/// Nor does a handle that is not one, or one whose halves disagree.
///
/// The two ways #16 says a URI is refused, and neither is checked here:
/// `src/handle.rs` decodes the segment and verifies it, so this resource
/// writes no parsing of its own. The edited handle below names one comparison
/// and describes another, which is what a client does when it changes the
/// package in a URI and leaves the identifier because the identifier looks
/// like the opaque part.
#[tokio::test]
async fn a_segment_that_is_not_a_handle_is_invalid_params() {
    for handle in [
        "not-a-handle".to_owned(),
        // The right prefix and a payload that is not base64.
        "d1:not-base64!".to_owned(),
        edited(&diffable(), "package", json!("elsewhere")),
        edited(&diffable(), "diff_id", json!("0".repeat(64))),
    ] {
        let uri = diff_uri(&handle);
        let answer = reading(&uri).await;

        assert_eq!(
            answer["error"]["code"], -32602,
            "`{handle}` is not a handle this server minted: got {answer}"
        );
    }
}

/// A comparison too large to serve whole says so, and says what to call.
///
/// The half of this resource that cannot be a shorter answer. A tree is whole
/// or it is misleading — an agent handed the first nine tenths of one has no
/// way to know the file it is looking for is in the last tenth — so what does
/// not fit comes back as what is *known* about it, with the tool that pages
/// through the same tree named beside it.
///
/// The fixture is not taken on trust. The answer carries the size it came to
/// and the ceiling it was measured against, and both are checked here against
/// the number the crate exports — so a fixture that stopped being big enough
/// fails rather than passing with nothing to prove.
#[tokio::test]
async fn a_comparison_too_large_to_serve_whole_says_so_and_says_what_to_call() {
    let handle = enormous();
    let document = read(&diff_uri(&handle)).await;
    let too_large = &document["tree_too_large"];

    assert!(
        too_large.is_object(),
        "this comparison does not fit in one answer, so it should say so: got {}",
        &document["totals"]
    );
    assert!(
        too_large["bytes"]
            .as_u64()
            .is_some_and(|bytes| bytes > page::PAYLOAD_CEILING as u64),
        "the fixture should be over the ceiling, or this test proves nothing: got {too_large}"
    );
    assert_eq!(
        too_large["ceiling"],
        json!(page::PAYLOAD_CEILING),
        "the answer should name the number it was measured against, got {too_large}"
    );

    // The part the criterion is actually about: not a short tree that reads
    // as a whole one.
    assert!(
        document["tree"].is_null(),
        "a tree that did not fit is absent, never cut and presented as complete"
    );

    // And what a reader has instead is enough to go on: what was compared,
    // how much changed, and the call that walks the rest.
    assert_eq!(document["inputs"]["package"], json!("enormous"));
    assert!(
        document["totals"]["unchanged"]
            .as_u64()
            .is_some_and(|files| files > 0),
        "the totals are what fits and should still be here, got {document}"
    );

    // The comparison has to be one. A tree of six thousand unchanged nodes is
    // the right size to reach this branch and says nothing about whether the
    // branch can carry one where something moved, so each status the fixture
    // produces is named — and a fixture quietly turned back into a version
    // against itself fails here.
    for status in ["added", "modified", "removed"] {
        assert!(
            document["totals"][status]
                .as_u64()
                .is_some_and(|files| files > 0),
            "the comparison that does not fit should have `{status}` files in \
             it, got {}",
            document["totals"]
        );
    }

    assert_eq!(too_large["read_with"], json!(TREE), "got {too_large}");

    // The pointer resolves. A tool named in a sentence is a sentence; one
    // that answers is a way forward.
    let page = call(TREE, json!({ "handle": handle, "limit": 1 })).await;
    assert!(
        page["structuredContent"]["total"]
            .as_u64()
            .is_some_and(|nodes| nodes > 0),
        "the tool the answer points at should walk this comparison, got {page}"
    );
}

/// A comparison that does fit carries no such statement.
///
/// The other half, and worth its own test: a document that always said the
/// tree was too large would pass the one above.
#[tokio::test]
async fn a_comparison_that_fits_is_served_whole() {
    let document = read(&diff_uri(&diffable())).await;

    assert!(
        document["tree_too_large"].is_null(),
        "this comparison fits, so nothing should be standing in for it: got {document}"
    );
    assert!(
        document["tree"].is_array(),
        "a comparison that fits is served whole, got {document}"
    );
}

// ---------------------------------------------------------------------------
// One file of one comparison
// ---------------------------------------------------------------------------

/// Reading one file's diff is what the tool returns at its defaults.
///
/// Three lines of context around each change, cut at the response ceiling if
/// it comes to that — the settings a caller gets by passing nothing but a
/// handle and a path, which is all a URI has room for.
#[tokio::test]
async fn reading_one_files_diff_is_what_the_tool_returns() {
    let handle = diffable();
    let path = "src/index.js";

    let contents = contents(&file_uri(&handle, path)).await;
    let patch = call(FILE_DIFF, json!({ "handle": handle, "path": path })).await;

    assert_eq!(
        contents[0]["text"], patch["structuredContent"]["text"],
        "the resource and the tool should render one file the same way, \
         got {contents:?} against {patch}"
    );
}

/// Whether the text is a patch is carried by the media type.
///
/// What the tool says in `isDiff`, a resource says in the one field a client
/// already reads to decide how to render something. A file both versions ship
/// byte for byte is its own content and a path neither version has is a
/// sentence; rendering either as a patch is how a reader ends up parsing
/// `@@` out of a file that has none.
#[tokio::test]
async fn a_text_that_is_not_a_patch_says_so_in_its_media_type() {
    let handle = diffable();

    for (path, media_type, is_diff) in [
        ("src/index.js", "text/x-diff", true),
        ("README.md", "text/plain", false),
        ("nowhere/at/all.js", "text/plain", false),
    ] {
        let contents = contents(&file_uri(&handle, path)).await;
        let patch = call(FILE_DIFF, json!({ "handle": handle, "path": path })).await;

        assert_eq!(
            contents[0]["mimeType"], media_type,
            "`{path}` should be served as {media_type}, got {contents:?}"
        );
        assert_eq!(
            patch["structuredContent"]["isDiff"],
            json!(is_diff),
            "the tool and the media type should agree about `{path}`, got {patch}"
        );
    }
}

/// A file that moved is diffed from where it was.
///
/// The tool takes `old_path` beside the path, and its own description tells a
/// caller to pass the one the tree gives. A URI has room for one path, so the
/// resource looks the other up in the comparison it already built — the
/// alternative being an answer that reports every line of a moved file as
/// added, which is confidently wrong about a file the package still ships and
/// which a reader has nothing in the answer to doubt with.
#[tokio::test]
async fn a_renamed_file_is_diffed_from_where_it_was() {
    let handle = diffable();
    let contents = contents(&file_uri(&handle, "src/new-name.js")).await;

    let patch = call(
        FILE_DIFF,
        json!({
            "handle": handle,
            "path": "src/new-name.js",
            "old_path": "src/old-name.js",
        }),
    )
    .await;

    assert_eq!(
        contents[0]["text"], patch["structuredContent"]["text"],
        "a rename should read as a rename, got {contents:?} against {patch}"
    );
}

/// A path encoded the way a URI template expands it reads the same file.
///
/// `{path}` is simple expansion under RFC 6570, which percent-encodes the
/// reserved characters — `/` among them. The values this template is for are
/// file paths, so very nearly every real expansion has a `/` in it, and a
/// client that follows the spec asks for `src%2Findex.js`. Read as it stands
/// that is a key no comparison has, so a conforming client would have been
/// told every file it asked about was in neither version.
///
/// Held against the unencoded form rather than against a literal patch: the
/// two spellings are one request, and what this is about is that they answer
/// alike.
#[tokio::test]
async fn a_path_encoded_the_way_a_uri_template_expands_it_reads_the_same_file() {
    let handle = diffable();

    let encoded = contents(&file_uri(&handle, "src%2Findex.js")).await;
    let plain = contents(&file_uri(&handle, "src/index.js")).await;

    assert_eq!(
        encoded[0]["text"], plain[0]["text"],
        "the two spellings of one path are one request, got {encoded:?} against {plain:?}"
    );
    assert_eq!(
        encoded[0]["mimeType"], plain[0]["mimeType"],
        "and the same answer about whether it is a patch, got {encoded:?}"
    );
}

/// An escape that is not one is refused rather than looked up.
///
/// A malformed URI and a file that is not there are different answers and a
/// client can tell them apart: this is `-32602` on the protocol channel,
/// where a path the comparison does not have is a successful read carrying
/// the sentence that says so. Decoding that quietly fell through to the
/// lookup would report the client's own broken URI as a missing file.
///
/// `%FF` is the other half: two hex digits that decode to a byte no UTF-8
/// string can hold. A path is text here, so that is malformed too rather
/// than something to render lossily.
#[tokio::test]
async fn a_path_whose_escapes_are_malformed_is_refused() {
    for path in ["src%2", "src%zz.js", "src/%FF.js"] {
        let answer = reading(&file_uri(&diffable(), path)).await;

        assert_eq!(
            answer["error"]["code"], -32602,
            "`{path}` is not a path this server can decode: got {answer}"
        );
        // Not "no resource at this URI": the URI is one of ours and the
        // client's own escape is what is wrong with it.
        assert!(
            answer["error"]["message"]
                .as_str()
                .is_some_and(|message| !message.contains("No resource at")),
            "the refusal should say what is wrong with the path, got {answer}"
        );
    }
}

/// A decoded path is a key in the comparison and never a path on a disk.
///
/// Decoding is what makes `%2F` a `/`, which is the one character that could
/// turn a lookup into a traversal if anything downstream ever opened a file
/// with it. Nothing does — the value is a `FileMap` key — and this is here so
/// that it stays that way: an encoded `../../etc/passwd` resolves to the
/// sentence a path no version has gets, the same as any other absent file.
///
/// Green before the decoding landed as well as after, which is the point: it
/// pins a property rather than proving a change.
#[tokio::test]
async fn an_encoded_traversal_is_a_missing_file_and_nothing_else() {
    let handle = diffable();

    let absent = contents(&file_uri(&handle, "nowhere/at/all.js")).await;

    for path in ["..%2F..%2Fetc%2Fpasswd", "%2Fetc%2Fpasswd"] {
        let traversal = contents(&file_uri(&handle, path)).await;

        assert_eq!(
            traversal[0]["text"], absent[0]["text"],
            "`{path}` is a key no comparison has, got {traversal:?}"
        );
        assert_eq!(
            traversal[0]["mimeType"], "text/plain",
            "and it is a sentence rather than a patch, got {traversal:?}"
        );
    }
}

/// A directory is refused rather than called absent.
///
/// The departure `get_file_diff` makes from the engine, inherited here: a
/// directory has no content, so the engine reads one as absent on both sides
/// and renders the sentence saying it is in neither version. That is false
/// about a path the package ships.
#[tokio::test]
async fn a_directory_is_refused_rather_than_called_absent() {
    let answer = reading(&file_uri(&diffable(), "src")).await;

    assert_eq!(
        answer["error"]["code"], -32602,
        "a directory has no diff to read, got {answer}"
    );
    // The message and not only the code: "no resource at this URI" is what a
    // path that never reached the comparison gets, and it would let this test
    // pass against a server that had not implemented the template at all.
    assert!(
        answer["error"]["message"]
            .as_str()
            .is_some_and(|message| message.contains("`src`") && message.contains("directory")),
        "the refusal should say the path is a directory, got {answer}"
    );
}

// ---------------------------------------------------------------------------
// Getting from a tool to a resource
// ---------------------------------------------------------------------------

/// The tool that makes a comparison links to the one that reads it back.
///
/// Without this a client has the handle and has to know how to build a URI
/// out of it, which is the client writing our URI format. With it, following
/// the answer is one hop and the format stays ours.
#[tokio::test]
async fn the_summary_links_to_the_comparison_it_made() {
    let summary = call(
        SUMMARY,
        json!({
            "registry": "npm",
            "package": "diffable",
            "from_version": "1.0.0",
            "to_version": "2.0.0",
        }),
    )
    .await;

    let handle = summary["structuredContent"]["handle"]
        .as_str()
        .unwrap_or_else(|| panic!("the summary carries a handle, got {summary}"));

    let link = summary["content"]
        .as_array()
        .unwrap_or_else(|| panic!("a result carries content, got {summary}"))
        .iter()
        .find(|block| block["type"] == "resource_link")
        .unwrap_or_else(|| panic!("the summary should carry a resource link, got {summary}"));

    assert_eq!(
        link["uri"],
        json!(diff_uri(handle)),
        "the link should point at the comparison this call made, got {link}"
    );

    // And it resolves, which is the half a URI in a string cannot promise.
    let document = read(link["uri"].as_str().expect("a link carries a URI")).await;
    assert_eq!(
        document["totals"], summary["structuredContent"]["totals"],
        "following the link should reach the comparison the summary described"
    );
}

/// The tools that only read one back do not.
///
/// A link is worth carrying where it is the way on from an answer. On a page
/// of a tree or one file's patch it would be the same URI repeated on every
/// call, pointing back at the thing the caller already has a handle for.
#[tokio::test]
async fn a_tool_that_reads_a_comparison_back_carries_no_link() {
    for tool in [TREE, FILE_DIFF] {
        let arguments = match tool {
            FILE_DIFF => json!({ "handle": diffable(), "path": "src/index.js" }),
            _ => json!({ "handle": diffable() }),
        };
        let result = call(tool, arguments).await;

        assert!(
            !result["content"]
                .as_array()
                .unwrap_or(&Vec::new())
                .iter()
                .any(|block| block["type"] == "resource_link"),
            "`{tool}` is already holding the handle it would link by, got {result}"
        );
    }
}

// ---------------------------------------------------------------------------
// What a client may hold on to
// ---------------------------------------------------------------------------

/// Every read says how long it may be treated as fresh, and who may keep it.
///
/// The criterion #11, #14 and #19 each arrived at and none could meet: in the
/// `2026-07-28` schema `CacheableResult` is extended by `ReadResourceResult`
/// and the list results, while `CallToolResult` extends plain `Result`. A
/// tool has nowhere to put a freshness hint; this is where the promise those
/// issues made comes due.
///
/// `public` throughout because this server has no authorization contexts to
/// keep apart — every caller is anonymous and gets the same answer, so an
/// intermediary holding one copy for everyone is correct rather than a leak.
#[tokio::test]
async fn every_read_says_how_fresh_it_is_and_who_may_cache_it() {
    for uri in readable().await {
        let result = result_of(&uri).await;

        assert!(
            result["ttlMs"].as_u64().is_some_and(|ttl| ttl > 0),
            "reading `{uri}` should say how long the answer stays fresh, got {result}"
        );
        assert_eq!(
            result["cacheScope"], "public",
            "reading `{uri}` is the same answer for every caller, got {result}"
        );
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Every URI this server will read, filled in where it is a template.
///
/// One list, so that a resource added without a freshness hint or without a
/// refusal of its own fails rather than going unasserted.
async fn readable() -> Vec<String> {
    vec![
        REGISTRIES.to_owned(),
        diff_uri(&diffable()),
        file_uri(&diffable(), "src/index.js"),
    ]
}

/// The URI one whole comparison is read at.
fn diff_uri(handle: &str) -> String {
    DIFF_TEMPLATE.replace("{handle}", handle)
}

/// The URI one file of one comparison is read at.
fn file_uri(handle: &str, path: &str) -> String {
    FILE_TEMPLATE
        .replace("{handle}", handle)
        .replace("{path}", path)
}

/// The tool that renders one file of a comparison.
const FILE_DIFF: &str = "get_file_diff";

/// A handle for the comparison that does not fit in one answer.
///
/// What makes this package large is the number and the length of its paths,
/// so 2.0.0 is three files rather than a second copy of six thousand:
/// everything it leaves out is a `removed` node in the tree, which keeps the
/// comparison over the ceiling and costs a kilobyte to check in.
///
/// It was a version against itself, and that was a tree of six thousand
/// *unchanged* nodes — the right size to exercise the response ceiling and
/// the wrong content to exercise anything else, since the branch that refuses
/// a tree too large to serve had never seen a node that moved.
fn enormous() -> String {
    handle("enormous", "1.0.0", "2.0.0")
}

/// A handle for `diffable` 1.0.0 → 2.0.0, minted rather than fetched.
///
/// The tool that mints one is called where the *agreement* between the two is
/// what is being asserted. Everywhere else a handle is just the segment, and
/// minting it here is no calls rather than one. The same helper
/// `tests/get_diff_tree.rs` has, for the same reason.
fn diffable() -> String {
    handle("diffable", "1.0.0", "2.0.0")
}

/// A handle for one npm comparison, at the defaults `diff_package_versions`
/// would have used.
fn handle(package: &str, from: &str, to: &str) -> String {
    DiffHandle::mint(Inputs {
        registry: Registry::Npm,
        package: package.to_owned(),
        from_version: from.to_owned(),
        to_version: to.to_owned(),
        similarity_threshold: 0.75,
        ignore_whitespace: false,
    })
    .encode()
}

/// The tool that mints what the diff resources take.
const SUMMARY: &str = "diff_package_versions";

/// The tool that walks a comparison's tree.
const TREE: &str = "get_diff_tree";

/// Every node of `handle`'s comparison, by following `get_diff_tree`'s
/// cursors to the end.
///
/// The tool rather than a second walk of the same tree: what is being
/// asserted is that the resource and the tool agree, which a tree this test
/// built itself could not show.
async fn walk(handle: &str) -> Vec<Value> {
    let mut nodes = Vec::new();
    let mut cursor: Option<String> = None;

    loop {
        let mut arguments = json!({ "handle": handle });
        if let Some(cursor) = &cursor {
            arguments["cursor"] = json!(cursor);
        }

        let page = call(TREE, arguments).await;
        let page = &page["structuredContent"];

        nodes.extend(
            page["items"]
                .as_array()
                .unwrap_or_else(|| panic!("a page carries items, got {page}"))
                .iter()
                .cloned(),
        );

        match page["nextCursor"].as_str() {
            Some(next) => cursor = Some(next.to_owned()),
            None => return nodes,
        }
    }
}

/// Call `tool` with `arguments`, returning the `result` — or panicking with
/// the JSON-RPC error, so a failure says what the server objected to.
async fn call(tool: &str, arguments: Value) -> Value {
    let answer = current(json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "tools/call",
        "params": { "name": tool, "arguments": arguments },
    }))
    .await;

    if let Some(error) = answer.get("error") {
        panic!("expected a result, got JSON-RPC error {error}");
    }
    answer["result"].clone()
}

/// The values a filled-in pattern is compared against the module at.
///
/// Every one is spelled in RFC 3986's unreserved characters, so the module
/// escapes none of them and a filled pattern is comparable with a URL it
/// built. The package name is unscoped, so both of npm's slots are the same
/// string — which is why the scoped case has a test of its own above.
const PACKAGE: &str = "serde";
const VERSION: &str = "1.0.0";
const QUERY: &str = "serde";

/// Below every source's own maximum, so it reaches a URL as it was asked for.
const LIMIT: u32 = 5;

/// `pattern` with every field it names filled in.
fn filled(pattern: &Value) -> String {
    pattern
        .as_str()
        .unwrap_or_else(|| panic!("a URL pattern is a string, got {pattern}"))
        .replace("{package-without-scope}", PACKAGE)
        .replace("{package}", PACKAGE)
        .replace("{version}", VERSION)
        .replace("{query}", QUERY)
        .replace("{limit}", &LIMIT.to_string())
}

/// The fields `value` carries, in the order a JSON object sorts them.
fn fields(value: &Value) -> Vec<&str> {
    value
        .as_object()
        .unwrap_or_else(|| panic!("expected an object, got {value}"))
        .keys()
        .map(String::as_str)
        .collect()
}

/// The registries a catalogue describes, in the order it describes them.
fn registries(catalogue: &Value) -> Vec<Value> {
    catalogue["registries"]
        .as_array()
        .unwrap_or_else(|| panic!("a catalogue describes registries, got {catalogue}"))
        .clone()
}

/// Read `uri`, returning the one document it answers with.
///
/// A resource answers with `contents`, which is a list because one URI can
/// stand for several documents. None of these does, so a read that came back
/// as more than one would be a resource that had quietly changed shape.
async fn read(uri: &str) -> Value {
    let contents = contents(uri).await;

    assert_eq!(
        contents.len(),
        1,
        "each of these URIs is one document, got {contents:?}"
    );

    let text = contents[0]["text"]
        .as_str()
        .unwrap_or_else(|| panic!("`{uri}` answers with text, got {:?}", contents[0]));

    serde_json::from_str(text)
        .unwrap_or_else(|e| panic!("`{uri}` answers with JSON, got {e}: {text}"))
}

/// The `contents` of a read, whatever shape they are in.
async fn contents(uri: &str) -> Vec<Value> {
    let result = result_of(uri).await;

    result["contents"]
        .as_array()
        .unwrap_or_else(|| panic!("a read answers with contents, got {result}"))
        .clone()
}

/// The whole `result` of a read — or a panic naming the JSON-RPC error, so a
/// failure says what the server objected to.
async fn result_of(uri: &str) -> Value {
    let answer = reading(uri).await;

    if let Some(error) = answer.get("error") {
        panic!("expected to read `{uri}`, got JSON-RPC error {error}");
    }

    answer["result"].clone()
}

/// The whole JSON-RPC answer to a read, error and all.
async fn reading(uri: &str) -> Value {
    current(json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "resources/read",
        "params": { "uri": uri },
    }))
    .await
}

/// `handle` with one field of its payload replaced.
///
/// What a client does to a handle it can read part of: change the thing it is
/// asking about and leave the identifier, which looks like an internal
/// detail. The result is a handle whose `diff_id` no longer names the inputs
/// beside it — and the encoding is spelled out here rather than taken from
/// the crate, so this is a forgery a client could send rather than one this
/// server helped build. The same helper `tests/get_diff_tree.rs` has.
fn edited(handle: &str, field: &str, value: Value) -> String {
    let encoded = handle
        .strip_prefix("d1:")
        .expect("a handle this server minted");
    let payload = URL_SAFE_NO_PAD
        .decode(encoded)
        .expect("a handle's payload is base64url");
    let mut payload: Value = serde_json::from_slice(&payload).expect("a handle's payload is JSON");

    payload[field] = value;

    format!("d1:{}", URL_SAFE_NO_PAD.encode(payload.to_string()))
}

/// Everything `resources/list` answers with.
async fn resources() -> Vec<Value> {
    let answer = current(json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "resources/list",
        "params": {},
    }))
    .await;

    answer["result"]["resources"]
        .as_array()
        .unwrap_or_else(|| panic!("resources/list should answer with an array, got {answer}"))
        .clone()
}

/// Everything `resources/templates/list` answers with.
async fn templates() -> Vec<Value> {
    let answer = current(json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "resources/templates/list",
        "params": {},
    }))
    .await;

    answer["result"]["resourceTemplates"]
        .as_array()
        .unwrap_or_else(|| {
            panic!("resources/templates/list should answer with an array, got {answer}")
        })
        .clone()
}

/// Every description a client is shown, and where it came from.
///
/// The two lists, and the link a tool's answer carries — which is the third
/// place a resource describes itself and the one no listing would reach.
async fn advertised() -> Vec<(String, String)> {
    let mut said = Vec::new();

    let listed = resources().await.into_iter().map(|resource| {
        let uri = resource["uri"].as_str().unwrap_or("<no uri>").to_owned();
        (uri, resource)
    });
    let templated = templates().await.into_iter().map(|template| {
        let uri = template["uriTemplate"]
            .as_str()
            .unwrap_or("<no uri>")
            .to_owned();
        (uri, template)
    });

    let summary = call(
        SUMMARY,
        json!({
            "registry": "npm",
            "package": "diffable",
            "from_version": "1.0.0",
            "to_version": "2.0.0",
        }),
    )
    .await;
    let linked = summary["content"]
        .as_array()
        .unwrap_or_else(|| panic!("a result carries content, got {summary}"))
        .iter()
        .filter(|block| block["type"] == "resource_link")
        .map(|link| (format!("`{SUMMARY}`'s link"), link.clone()))
        .collect::<Vec<_>>();

    for (where_from, described) in listed.chain(templated).chain(linked) {
        for field in ["title", "description"] {
            if let Some(text) = described[field].as_str() {
                said.push((format!("{where_from} {field}"), text.to_owned()));
            }
        }
    }

    assert!(
        !said.is_empty(),
        "this file's other tests would all have to be failing for there to be nothing here"
    );

    said
}

/// A request as a conforming `2026-07-28` client sends it, to a server whose
/// archives come from `fixtures/archives/` rather than from the registries.
///
/// Named for the revision rather than for the verb, because the only thing
/// this and [`previous`] differ in is which revision the client speaks —
/// `tests/common/mod.rs` owns everything else about sending it, and a second
/// `post` in `tests/` would read as a second opinion about that.
async fn current(body: Value) -> Value {
    Client::fixture().post(body).await
}

/// The same, as a client on the previous revision sends it: none of the
/// headers or the `_meta` SEP-2243 and SEP-1319 added.
async fn previous(body: Value) -> Value {
    Client::fixture().speaking(PREVIOUS).post(body).await
}
