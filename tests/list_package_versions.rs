//! `list_package_versions`, driven the way an agent drives it.
//!
//! The same two seams as `tests/list_package_files.rs`: most of what is here
//! goes over the wire, because a test that only called the handler would keep
//! passing while the definition beside it stopped matching. The handler is
//! reached directly only where the question is about the answer's *type*.
//!
//! The documents below are `fixtures/versions/`, keyed by the URL
//! `src/registry.rs` builds, so a fetch that built a URL of its own finds
//! nothing.
//!
//! # What is deliberately not asserted here
//!
//! That this answer carries `ttlMs` and `cacheScope`. #18 asks for both, and
//! a `tools/call` result has nowhere to put them: in the `2026-07-28` schema
//! `CacheableResult` is extended by `DiscoverResult`, the four list results
//! and `ReadResourceResult`, and `CallToolResult` extends plain `Result`. The
//! freshness hint this tool wants is a resource's to carry, which is #16;
//! what `tools/list` carries is `src/mcp.rs`'s and `tests/mcp.rs` holds it.
//!
//! That nothing here reaches the blob store. There is no store yet (#20,
//! #21), and when there is one `scripts/check-tool-seams.sh` is what keeps a
//! tool module from naming it — a rule held over every tool rather than a
//! fact about this one. `src/catalogue/` has no store in it at all, which is
//! the part that matters: registry metadata goes stale on its own and the
//! 256 MB budget belongs to diff results.
//!
//! That a walk of a sequence is stable, that a page stays under the response
//! ceiling, and that a cursor of a client's own invention is refused. Those
//! are `src/page.rs`'s and `tests/page.rs` holds them against a generated
//! sequence. What this suite asserts is that this tool goes *through* that
//! module rather than around it.

use axum::body::Body;
use axum::http::Request;
use diffpack_server::catalogue::Catalogue;
use diffpack_server::mcp::Diffpack;
use diffpack_server::router;
use diffpack_server::tools::Ctx;
use http_body_util::BodyExt;
use serde_json::{json, Value};
use tower::ServiceExt;

const CURRENT: &str = "2026-07-28";

const TOOL: &str = "list_package_versions";

/// The version documents this suite is served from, instead of the registries.
const FIXTURES: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/fixtures/versions");

// ---------------------------------------------------------------------------
// What it answers
// ---------------------------------------------------------------------------

/// The tracer bullet: one npm package, newest release first.
///
/// `zod`'s fixture is built so that the right answer is not reachable by
/// accident. Its newest release is `1.0.2` — a patch to the 1.x line
/// published after 2.0.0 was, which is what npm's own `@types/node` does
/// every week. So the expected order below is not the semver order, not the
/// lexical order of the keys, and not the order the document is written in.
/// A listing that took any of those three would have to disagree with it.
#[tokio::test]
async fn an_npm_packages_versions_come_back_newest_first() {
    let result = call(json!({
        "registry": "npm",
        "package": "zod",
    }))
    .await;

    assert_eq!(
        result["isError"],
        json!(false),
        "a package the fixture set has is not an error, got {result}"
    );

    assert_eq!(
        versions(&result),
        vec!["1.0.2", "1.0.10", "2.0.0", "1.0.0"],
        "newest first means most recently published first: 1.0.2 is a patch \
         to the 1.x line published after 2.0.0, so a listing sorted by \
         version number or by the document's own order gets this wrong: got \
         {result}"
    );
}

/// crates.io answers with an array where npm answers with an object, and its
/// dates are spelled to the microsecond where npm's are to the millisecond.
/// Out the other side they are the same listing.
///
/// The five releases below are `tokio`'s real ones, dates included, and they
/// are here because of what they are not: 1.51.4 was published between 1.52.4
/// and 1.52.3, so the order by date is not the order by version number. A
/// listing that sorted on the number would put 1.52.3 above 1.51.4.
#[tokio::test]
async fn a_crates_io_packages_versions_come_back_newest_first() {
    let result = call(json!({
        "registry": "crates",
        "package": "tokio",
    }))
    .await;

    assert_eq!(
        result["isError"],
        json!(false),
        "a crate the fixture set has is not an error, got {result}"
    );

    assert_eq!(
        versions(&result),
        vec!["1.53.1", "1.53.0", "1.52.4", "1.51.4", "1.52.3"],
        "a backport published between two releases of a newer line sits where \
         its date puts it, not where its version number would: got {result}"
    );
}

/// PyPI, through deps.dev, and the case that says why the order is computed
/// from a date rather than read off the document.
///
/// deps.dev sorts a package's versions **lexically by version string**, which
/// is neither newest-first nor oldest-first. The five releases below are
/// `requests`' real ones in deps.dev's real order, and `2.9.2` is last
/// because `"2.9.2"` sorts after `"2.34.2"`. So a listing that reversed the
/// document — which is what this tool's issue originally specified — would
/// announce a 2016 release as the newest version of `requests`.
#[tokio::test]
async fn a_pypi_packages_versions_are_ordered_by_date_not_by_the_documents_order() {
    let result = call(json!({
        "registry": "pypi",
        "package": "requests",
    }))
    .await;

    assert_eq!(
        result["isError"],
        json!(false),
        "a package the fixture set has is not an error, got {result}"
    );

    assert_eq!(
        versions(&result),
        vec!["2.34.2", "2.31.0", "2.9.2", "2.9.0", "0.10.0"],
        "deps.dev lists these lexically, so the last entry is 2.9.2 and \
         reversing the document would put a 2016 release on top: got {result}"
    );
}

/// A scoped npm name is one package name, so it is one escaped path segment.
/// The fixture set is keyed by the URL, so a listing that interpolated
/// `@types/node` into a path — and so asked npm for a package called `node`
/// inside a scope — finds nothing here.
///
/// The five releases are `@types/node`'s real ones and are what makes the
/// case for ordering by date rather than by version number: npm's own listing
/// shows 24.13.6 on top, published forty seconds after 22.20.4 and three days
/// after 24.13.5, while `dist-tags.latest` is 26.6.2. Three different
/// questions, and this tool answers the one an agent asked.
#[tokio::test]
async fn a_scoped_npm_package_is_asked_for_under_its_whole_name() {
    let result = call(json!({
        "registry": "npm",
        "package": "@types/node",
    }))
    .await;

    assert_eq!(
        result["isError"],
        json!(false),
        "a scoped package the fixture set has is not an error, got {result}"
    );

    assert_eq!(
        versions(&result),
        vec!["24.13.6", "22.20.4", "25.9.8", "26.6.2", "24.13.5"],
        "the newest release of this package is a 24.x patch and the second \
         newest is a 22.x one, which is neither the semver order nor the \
         `latest` tag: got {result}"
    );
}

// ---------------------------------------------------------------------------
// Which of them are previews
// ---------------------------------------------------------------------------

/// An agent asked for "the last two versions" should not silently diff
/// against a release candidate, so every entry says whether it is one.
///
/// npm and crates.io spell a version the way semver does, so what follows the
/// first `-` is the prerelease. Build metadata is not: `2.0.1+build.5` is the
/// same release as `2.0.1` with a label on it, and flagging it would tell an
/// agent to avoid the newest stable release there is.
#[tokio::test]
async fn an_npm_prerelease_is_flagged_and_build_metadata_is_not() {
    let result = call(json!({
        "registry": "npm",
        "package": "prereleases",
    }))
    .await;

    assert_eq!(
        previews(&result),
        vec![
            ("2.0.1+build.5", false),
            ("2.0.0", false),
            ("2.0.0-rc.1", true),
            ("2.0.0-alpha.1", true),
            ("1.0.0", false),
        ],
        "semver's prerelease is what follows the first `-`, and `+build.5` is \
         not one: got {result}"
    );
}

/// PyPI is not semver, and the difference is not cosmetic: PEP 440 glues its
/// markers straight onto the release, so `1.0rc1` is a release candidate and
/// there is no `-` anywhere in it. Reading it with npm's rule flags nothing.
///
/// `1.0.post1` is the other half. A post-release is a re-release of `1.0` —
/// a fixed description, a corrected classifier — not a preview of anything,
/// and flagging it would point an agent away from the newest thing there is.
#[tokio::test]
async fn a_pypi_prerelease_is_flagged_and_a_post_release_is_not() {
    let result = call(json!({
        "registry": "pypi",
        "package": "prereleases",
    }))
    .await;

    assert_eq!(
        previews(&result),
        vec![
            ("1.0.post1", false),
            ("1.0", false),
            ("1.0rc1", true),
            ("1.0b2", true),
            ("1.0a1", true),
            ("1.0.dev1", true),
        ],
        "PEP 440 needs no separator before its marker, and a post-release is \
         not a preview: got {result}"
    );
}

// ---------------------------------------------------------------------------
// Reading it a page at a time
// ---------------------------------------------------------------------------

/// That a walk is stable — every version once, none missed, none repeated —
/// is `src/page.rs`'s property and `tests/page.rs` proves it against a
/// generated sequence. What is left for this tool is the part only it can be
/// wrong about: that the sequence reaches that module at all, so `limit` is
/// honoured and the cursor handed back is the one that resumes the walk.
#[tokio::test]
async fn a_walk_of_the_pages_is_the_whole_listing() {
    let whole: Vec<String> =
        versions(&call(json!({ "registry": "pypi", "package": "requests" })).await)
            .iter()
            .map(|version| version.to_string())
            .collect();

    let first = call(json!({
        "registry": "pypi", "package": "requests", "limit": 2,
    }))
    .await;

    assert_eq!(
        versions(&first),
        &whole[..2],
        "a page of two is the first two, got {first}"
    );
    assert_eq!(
        first["structuredContent"]["total"],
        json!(5),
        "the total is how many versions the package has and not how many are \
         on this page — an agent told it received 2 of 2 has no reason to ask \
         again: got {first}"
    );

    let cursor = first["structuredContent"]["nextCursor"]
        .as_str()
        .unwrap_or_else(|| panic!("a page that ends early hands back a cursor, got {first}"))
        .to_owned();

    let rest = call(json!({
        "registry": "pypi", "package": "requests", "cursor": cursor,
    }))
    .await;

    assert_eq!(
        versions(&rest),
        &whole[2..],
        "the cursor resumes where the page stopped, got {rest}"
    );
    assert!(
        rest["structuredContent"]["nextCursor"].is_null(),
        "the last page of a sequence does not hand out another cursor, got {rest}"
    );
}

// ---------------------------------------------------------------------------
// When there is nothing to list
// ---------------------------------------------------------------------------

/// A package the registry does not have is something the model can act on —
/// it misspelled a name or picked the wrong registry — so it arrives as a
/// tool error it reads rather than as a protocol error it never sees.
///
/// It is the *package* that is missing and not a version, which is the whole
/// of what makes this seam's refusal different from the archive's: there is
/// no version in the request to have got wrong, so the message does not
/// suggest checking one.
#[tokio::test]
async fn a_package_the_registry_does_not_have_is_a_tool_error_naming_it() {
    let result = call(json!({
        "registry": "npm",
        "package": "not-a-real-package",
    }))
    .await;

    assert_eq!(
        result["isError"],
        json!(true),
        "a package that is not there is the model's to act on, got {result}"
    );

    let message = result["content"][0]["text"]
        .as_str()
        .unwrap_or_else(|| panic!("a tool error carries text a model reads, got {result}"));

    assert!(
        message.contains("not-a-real-package") && message.contains("npm"),
        "the message names what was asked for and where, got {message}"
    );
}

// ---------------------------------------------------------------------------
// Reading the answer
// ---------------------------------------------------------------------------

/// Each entry's version and whether it is a preview, in the order returned.
fn previews(result: &Value) -> Vec<(&str, bool)> {
    result["structuredContent"]["items"]
        .as_array()
        .unwrap_or_else(|| panic!("a page carries its items, got {result}"))
        .iter()
        .map(|entry| {
            let version = entry["version"]
                .as_str()
                .unwrap_or_else(|| panic!("every entry names a version, got {entry}"));
            let prerelease = entry["prerelease"].as_bool().unwrap_or_else(|| {
                panic!("every entry says whether it is a prerelease, got {entry}")
            });
            (version, prerelease)
        })
        .collect()
}

/// The `version` of every entry on this page, in the order they were returned.
fn versions(result: &Value) -> Vec<&str> {
    result["structuredContent"]["items"]
        .as_array()
        .unwrap_or_else(|| panic!("a page carries its items, got {result}"))
        .iter()
        .map(|entry| {
            entry["version"]
                .as_str()
                .unwrap_or_else(|| panic!("every entry names a version, got {entry}"))
        })
        .collect()
}

/// Call the tool with `arguments`, returning the `result` — or panicking with
/// the JSON-RPC error, so a failure says what the server objected to.
async fn call(arguments: Value) -> Value {
    let answer = post(json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "tools/call",
        "params": { "name": TOOL, "arguments": arguments, "_meta": meta() },
    }))
    .await;

    if let Some(error) = answer.get("error") {
        panic!("expected a result, got JSON-RPC error {error}");
    }
    answer["result"].clone()
}

/// The per-request `_meta` a `2026-07-28` client attaches. See `tests/mcp.rs`.
fn meta() -> Value {
    json!({
        "io.modelcontextprotocol/protocolVersion": CURRENT,
        "io.modelcontextprotocol/clientCapabilities": {},
    })
}

/// A request as a conforming `2026-07-28` client sends it, to a server whose
/// version documents come from `fixtures/versions/` rather than from the
/// registries.
async fn post(body: Value) -> Value {
    let method = body["method"].as_str().expect("a call names a method");

    let mut request = Request::builder()
        .method("POST")
        .uri("/mcp")
        .header("host", "mcp.diffpack.io")
        .header("accept", "application/json, text/event-stream")
        .header("content-type", "application/json")
        .header("mcp-protocol-version", CURRENT)
        .header("mcp-method", method);

    if let Some(name) = body["params"]["name"].as_str() {
        request = request.header("mcp-name", name);
    }

    let request = request
        .body(Body::from(body.to_string()))
        .expect("the request should build");

    let router = router::router_with(
        || {
            Ok(Diffpack::with_ctx(Ctx::with_catalogue(Catalogue::fixture(
                FIXTURES,
            ))))
        },
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

    serde_json::from_slice(&bytes).unwrap_or_else(|e| {
        panic!(
            "expected a JSON body ({status}), got {e}: {}",
            String::from_utf8_lossy(&bytes)
        )
    })
}
