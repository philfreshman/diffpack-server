mod common;

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine as _;
use common::{names, Client, PREVIOUS};
use diffpack_server::handle::{DiffHandle, Inputs};
use diffpack_server::page;
use diffpack_server::registry::{ArchiveSource, Registry, VERSION_RULE};
use serde_json::{json, Value};

const REGISTRIES: &str = "diffpack://registries";

const DIFF_TEMPLATE: &str = "diffpack://diff/{handle}";

const FILE_TEMPLATE: &str = "diffpack://diff/{handle}/file/{path}";

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

    let description = catalogue["description"].as_str().unwrap_or_default();
    for registry in Registry::ALL {
        assert!(
            names(description, registry.name()),
            "the description says which registries the catalogue holds, so it \
             should name {}: got {description}",
            registry.name()
        );
    }
}

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
        vec!["npm", "crates", "pypi", "go"],
        "all four registries should be described, got {catalogue}"
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
            "https://proxy.golang.org/{package}/@v/{version}.zip",
        ],
        "each registry should say where a version's archive comes from, got {catalogue}"
    );
}

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

        let versions = registry.versions(PACKAGE);
        let mut expected = vec!["url_pattern"];
        if versions.current.is_some() {
            expected.insert(0, "current_url_pattern");
        }
        assert_eq!(
            fields(&described["versions"]),
            expected,
            "a registry whose Current version is a second document says where, \
             and no other does: got {described}"
        );
        assert_eq!(
            filled(&described["versions"]["url_pattern"]),
            versions.url,
            "filling `{id}`'s versions pattern in should give the URL the module builds"
        );
        if let Some(current) = versions.current {
            assert_eq!(
                filled(&described["versions"]["current_url_pattern"]),
                current,
                "filling `{id}`'s current-version pattern in should give the URL \
                 the module builds"
            );
        }

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

#[tokio::test]
async fn a_comparison_nothing_has_computed_is_recomputed_rather_than_refused() {
    let document = read(&diff_uri(&diffable())).await;

    assert_eq!(
        document["totals"]["modified"],
        json!(1),
        "a comparison nobody has made yet is one this server can make: got {document}"
    );
}

#[tokio::test]
async fn a_uri_this_server_does_not_serve_is_invalid_params() {
    for uri in [
        "diffpack://nothing-here",
        "https://example.invalid/diff",
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

#[tokio::test]
async fn a_version_the_registry_does_not_have_is_not_a_uri_that_is_not_ours() {
    let failed = reading(&diff_uri(&unpublished())).await;
    let stranger = reading("diffpack://nothing-here").await;

    assert_eq!(
        failed["error"]["code"], -32001,
        "a version the registry does not have is a failure of the comparison, \
         not a URI this server does not serve: got {failed}"
    );
    assert_eq!(
        stranger["error"]["code"], -32602,
        "a URI that resolves to nothing is the caller's to fix: got {stranger}"
    );
    assert_ne!(
        failed["error"]["code"], stranger["error"]["code"],
        "a read has one channel, so the code is the whole of what a client has \
         to tell these two apart"
    );
}

#[tokio::test]
async fn a_segment_that_is_not_a_handle_is_invalid_params() {
    for handle in [
        "not-a-handle".to_owned(),
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

    assert!(
        document["tree"].is_null(),
        "a tree that did not fit is absent, never cut and presented as complete"
    );

    assert_eq!(document["inputs"]["package"], json!("enormous"));
    assert!(
        document["totals"]["unchanged"]
            .as_u64()
            .is_some_and(|files| files > 0),
        "the totals are what fits and should still be here, got {document}"
    );

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

    let page = call(TREE, json!({ "handle": handle, "limit": 1 })).await;
    assert!(
        page["structuredContent"]["total"]
            .as_u64()
            .is_some_and(|nodes| nodes > 0),
        "the tool the answer points at should walk this comparison, got {page}"
    );
}

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

#[tokio::test]
async fn a_path_whose_escapes_are_malformed_is_refused() {
    let handle = diffable();

    let works = reading(&file_uri(&handle, "src/index.js")).await;
    assert!(
        works.get("result").is_some(),
        "this template reads a path that is one, so a refusal below is the \
         decoder's: got {works}"
    );

    for path in ["src%2", "src%zz.js", "src/%FF.js"] {
        let answer = reading(&file_uri(&handle, path)).await;

        assert_eq!(
            answer["error"]["code"], -32602,
            "`{path}` is not a path this server can decode: got {answer}"
        );
    }
}

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

#[tokio::test]
async fn a_directory_is_refused_rather_than_called_absent() {
    let answer = reading(&file_uri(&diffable(), "src")).await;

    assert_eq!(
        answer["error"]["code"], -32001,
        "a directory has no diff to read, and that is the comparison's answer \
         rather than a URI this server does not serve: got {answer}"
    );
}

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

    let document = read(link["uri"].as_str().expect("a link carries a URI")).await;
    assert_eq!(
        document["totals"], summary["structuredContent"]["totals"],
        "following the link should reach the comparison the summary described"
    );
}

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

async fn readable() -> Vec<String> {
    vec![
        REGISTRIES.to_owned(),
        diff_uri(&diffable()),
        file_uri(&diffable(), "src/index.js"),
    ]
}

fn diff_uri(handle: &str) -> String {
    DIFF_TEMPLATE.replace("{handle}", handle)
}

fn file_uri(handle: &str, path: &str) -> String {
    FILE_TEMPLATE
        .replace("{handle}", handle)
        .replace("{path}", path)
}

const FILE_DIFF: &str = "get_file_diff";

fn enormous() -> String {
    handle("enormous", "1.0.0", "2.0.0")
}

fn diffable() -> String {
    handle("diffable", "1.0.0", "2.0.0")
}

fn unpublished() -> String {
    handle("diffable", "1.0.0", "9.9.9")
}

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

const SUMMARY: &str = "diff_package_versions";

const TREE: &str = "get_diff_tree";

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

const PACKAGE: &str = "serde";
const VERSION: &str = "1.0.0";
const QUERY: &str = "serde";

const LIMIT: u32 = 5;

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

fn fields(value: &Value) -> Vec<&str> {
    value
        .as_object()
        .unwrap_or_else(|| panic!("expected an object, got {value}"))
        .keys()
        .map(String::as_str)
        .collect()
}

fn registries(catalogue: &Value) -> Vec<Value> {
    catalogue["registries"]
        .as_array()
        .unwrap_or_else(|| panic!("a catalogue describes registries, got {catalogue}"))
        .clone()
}

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

async fn contents(uri: &str) -> Vec<Value> {
    let result = result_of(uri).await;

    result["contents"]
        .as_array()
        .unwrap_or_else(|| panic!("a read answers with contents, got {result}"))
        .clone()
}

async fn result_of(uri: &str) -> Value {
    let answer = reading(uri).await;

    if let Some(error) = answer.get("error") {
        panic!("expected to read `{uri}`, got JSON-RPC error {error}");
    }

    answer["result"].clone()
}

async fn reading(uri: &str) -> Value {
    current(json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "resources/read",
        "params": { "uri": uri },
    }))
    .await
}

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

async fn current(body: Value) -> Value {
    Client::fixture().post(body).await
}

async fn previous(body: Value) -> Value {
    Client::fixture().speaking(PREVIOUS).post(body).await
}
