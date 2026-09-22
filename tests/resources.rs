//! The resources, driven the way a client drives them.
//!
//! One seam: the wire. `resources/list`, `resources/templates/list` and
//! `resources/read` through `router_with` over the fixture archives, which is
//! how `tests/get_diff_tree.rs` drives the tools. A resource is a URI and
//! nothing else — there is no schema a client reads first — so the URI
//! reaching the code that answers it is most of what can break, and a handler
//! test would pass through a URI that never matched.

use axum::body::Body;
use axum::http::Request;
use diffpack_server::mcp::Diffpack;
use diffpack_server::registry::{ArchiveSource, Registry, VERSION_RULE};
use diffpack_server::router;
use diffpack_server::tools::Ctx;
use http_body_util::BodyExt;
use serde_json::{json, Value};
use tower::ServiceExt;

const CURRENT: &str = "2026-07-28";

/// The fixture sets this suite is served from, instead of the registries.
const FIXTURES: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/fixtures");

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
    let answer = post(json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "server/discover",
        "params": { "_meta": meta() },
    }))
    .await;

    assert!(
        answer["result"]["capabilities"]["resources"].is_object(),
        "a server offering resources should say so where a client looks, got {}",
        answer["result"]["capabilities"]
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
/// refusal of its own fails rather than going unasserted. It grows as the
/// templates become fillable.
async fn readable() -> Vec<String> {
    vec![REGISTRIES.to_owned()]
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
    let answer = post(json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "resources/read",
        "params": { "uri": uri, "_meta": meta() },
    }))
    .await;

    if let Some(error) = answer.get("error") {
        panic!("expected to read `{uri}`, got JSON-RPC error {error}");
    }

    answer["result"].clone()
}

/// Everything `resources/list` answers with.
async fn resources() -> Vec<Value> {
    let answer = post(json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "resources/list",
        "params": { "_meta": meta() },
    }))
    .await;

    answer["result"]["resources"]
        .as_array()
        .unwrap_or_else(|| panic!("resources/list should answer with an array, got {answer}"))
        .clone()
}

/// Everything `resources/templates/list` answers with.
async fn templates() -> Vec<Value> {
    let answer = post(json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "resources/templates/list",
        "params": { "_meta": meta() },
    }))
    .await;

    answer["result"]["resourceTemplates"]
        .as_array()
        .unwrap_or_else(|| {
            panic!("resources/templates/list should answer with an array, got {answer}")
        })
        .clone()
}

/// The per-request `_meta` a `2026-07-28` client attaches. See `tests/mcp.rs`.
fn meta() -> Value {
    json!({
        "io.modelcontextprotocol/protocolVersion": CURRENT,
        "io.modelcontextprotocol/clientCapabilities": {},
    })
}

/// A request as a conforming `2026-07-28` client sends it, to a server whose
/// archives come from `fixtures/archives/` rather than from the registries.
async fn post(body: Value) -> Value {
    let (_, answer) = respond(body).await;
    answer
}

/// The same request, with the body the client received beside the answer.
///
/// The bytes rather than the structure, because that is what the response
/// ceiling bounds: what Vercel refuses is the frame this server wrote, and
/// two frames that parse alike can still differ.
async fn respond(body: Value) -> (String, Value) {
    let method = body["method"].as_str().expect("a call names a method");

    let mut request = Request::builder()
        .method("POST")
        .uri("/mcp")
        .header("host", "mcp.diffpack.io")
        .header("accept", "application/json, text/event-stream")
        .header("content-type", "application/json")
        .header("mcp-protocol-version", CURRENT)
        .header("mcp-method", method);

    // SEP-2243 repeats the thing being asked for in a header as well as in
    // the body, so an intermediary can route without parsing one. For a read
    // that is the URI, the way it is the tool name for a call — the client's
    // obligation under the transport, not this server's leniency, which is
    // why the builder always meets it.
    for source in ["uri", "name"] {
        if let Some(name) = body["params"][source].as_str() {
            request = request.header("mcp-name", name);
        }
    }

    let request = request
        .body(Body::from(body.to_string()))
        .expect("the request should build");

    let router = router::router_with(
        || Ok(Diffpack::with_ctx(Ctx::fixture(FIXTURES))),
        Vec::new(),
    );

    let response = router
        .oneshot(request)
        .await
        .expect("the router answers every request");

    let status = response.status();
    let bytes = response
        .into_body()
        .collect()
        .await
        .expect("the body should read")
        .to_bytes();

    let answer = serde_json::from_slice(&bytes).unwrap_or_else(|e| {
        panic!(
            "expected a JSON body ({status}), got {e}: {}",
            String::from_utf8_lossy(&bytes)
        )
    });

    (String::from_utf8_lossy(&bytes).into_owned(), answer)
}
