//! One line per tool call, driven the way an agent drives it.
//!
//! The line is what an incident is read from, so what it has to carry is
//! fixed here rather than in whichever tool happened to be written first:
//! which tool ran, what it was asked for, how long it took and how it ended.
//!
//! Every test goes over the wire. A test that called the emitter directly
//! would keep passing on the day a tool stopped reaching it, and "every tool
//! call emits one line" is exactly the property that fails that way — so the
//! sink is handed to the [`Ctx`] the service factory builds, and the line
//! asserted on is the one a real `tools/call` produced.

use axum::body::Body;
use axum::http::Request;
use diffpack_server::archive::Archive;
use diffpack_server::log::Capture;
use diffpack_server::mcp::Diffpack;
use diffpack_server::router;
use diffpack_server::tools::Ctx;
use http_body_util::BodyExt;
use serde_json::{json, Value};
use tower::ServiceExt;

const CURRENT: &str = "2026-07-28";

/// The archives this suite is served from, instead of the registries.
const FIXTURES: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/fixtures/archives");

/// The question every other assertion here depends on: a call that went
/// through the endpoint left exactly one line behind, and that line says
/// which tool it was about.
///
/// One and not "at least one" on purpose. Two lines for one call is the shape
/// that makes a rate of anything uncountable later, and it is the shape a
/// second emitter added in a hurry produces.
#[tokio::test]
async fn a_tool_call_emits_one_line_naming_the_tool() {
    let log = Capture::new();

    call(
        &log,
        "list_package_files",
        json!({ "registry": "npm", "package": "@types/node", "version": "20.1.0" }),
    )
    .await;

    let lines = log.lines();
    assert_eq!(
        lines.len(),
        1,
        "one call should leave one line behind, got {lines:?}"
    );

    let line = parse(&lines[0]);
    assert_eq!(
        line["tool"], "list_package_files",
        "the first question asked in an incident is which tool ran, got {line}"
    );
}

/// A call that worked says so, in a word rather than by the absence of
/// anything else. "How did it end" is the question a line is filtered on, so
/// the successful calls have to be as findable as the failed ones — a rate is
/// two counts, and one of them is this.
#[tokio::test]
async fn a_call_that_worked_ends_ok() {
    let log = Capture::new();

    call(
        &log,
        "list_package_files",
        json!({ "registry": "npm", "package": "@types/node", "version": "20.1.0" }),
    )
    .await;

    assert_eq!(
        one(&log)["result"],
        "ok",
        "a call that answered should say so, got {}",
        one(&log)
    );
}

/// A call that failed names which failure, not that there was one.
///
/// "Error rate by cause" is a rate per cause, and a line that said only
/// `error` would leave every cause in one bucket — a registry that is down,
/// a package nobody has, and an archive too big to hold are three different
/// incidents with three different responses.
#[tokio::test]
async fn a_call_that_failed_names_the_cause() {
    let log = Capture::new();

    call(
        &log,
        "list_package_files",
        // The fixture set answers this URL with nothing, which is the path a
        // registry's 404 takes.
        json!({ "registry": "npm", "package": "zod", "version": "99.99.99" }),
    )
    .await;

    assert_eq!(
        one(&log)["result"],
        "no_such_version",
        "the cause is what an error rate is broken down by, got {}",
        one(&log)
    );
}

/// Which registry, which package, which versions — the questions asked
/// straight after "which tool", and the ones that decide whether an incident
/// is this server's or a registry's.
///
/// Taken from the arguments as they arrived rather than from a list this
/// module keeps of what matters. A per-tool list is a list to forget to widen,
/// and the tool whose argument nobody thought to log is the one being looked
/// for.
#[tokio::test]
async fn the_line_says_what_was_asked_for() {
    let log = Capture::new();

    call(
        &log,
        "list_package_files",
        json!({ "registry": "npm", "package": "@types/node", "version": "20.1.0" }),
    )
    .await;

    let line = one(&log);
    assert_eq!(
        line["args"],
        json!({ "registry": "npm", "package": "@types/node", "version": "20.1.0" }),
        "the line should carry what the call was for, got {line}"
    );
}

/// A summary, not a copy: a long argument is cut, and says it was.
///
/// A cursor and a diff handle are both opaque and both arbitrarily long, and
/// a line that carried one whole would push everything worth reading off the
/// end of it. Cut rather than dropped, because the first characters still
/// tell two calls apart, and marked rather than silently shortened, because a
/// value that looks complete and is not is worse than one that admits it.
#[tokio::test]
async fn a_long_argument_is_cut_and_says_so() {
    let log = Capture::new();
    let long = "x".repeat(500);

    call(
        &log,
        "list_package_files",
        json!({
            "registry": "npm",
            "package": "@types/node",
            "version": "20.1.0",
            "cursor": long,
        }),
    )
    .await;

    let line = one(&log);
    let logged = line["args"]["cursor"]
        .as_str()
        .unwrap_or_else(|| panic!("the cursor should still be in the line, got {line}"));

    assert!(
        logged.len() < long.len(),
        "a 500 byte argument should not arrive whole, got {} bytes",
        logged.len()
    );
    assert!(
        logged.ends_with('\u{2026}'),
        "a value that was cut should say so rather than look complete, got {logged}"
    );
    assert!(
        long.starts_with(logged.trim_end_matches('\u{2026}')),
        "what is kept should be the start of what was sent, got {logged}"
    );
}

// ---------------------------------------------------------------------------
// What never reaches a line
// ---------------------------------------------------------------------------

/// A line is written where a person will read it and a platform will keep it,
/// which makes it the one place a credential must not turn up.
///
/// The arguments are the surface: they arrive from whoever called, and a
/// caller who put a signed URL in a package name would otherwise have written
/// it into our runtime logs. `tests/errors.rs` holds the same rule over what
/// reaches a client; this holds it over what reaches an operator, and the two
/// go through one redactor so they cannot disagree about what a secret is.
#[tokio::test]
async fn no_line_carries_a_credential_or_a_signed_url() {
    // Assembled rather than written out, for the reason `tests/errors.rs`
    // gives: a literal of this shape is a secret scanner's false positive on
    // every pull request that touches the file. Do not tidy it into one
    // literal.
    let blob_token = ["vercel", "blob", "rw", "A1b2C3d4E5f6G7h8i9J0kL1mN2oP3qR4"].join("_");

    let secrets = [
        blob_token.as_str(),
        "https://blob.vercel-storage.com/diffs/abc?token=s3cr3t&expires=1",
        "/var/task/diffpack-server/src/cache.rs",
    ];

    for secret in secrets {
        let log = Capture::new();

        call(
            &log,
            "list_package_files",
            json!({ "registry": "npm", "package": secret, "version": "20.1.0" }),
        )
        .await;

        let line = one(&log).to_string();
        assert!(
            !line.contains(secret),
            "`{secret}` survived into a log line: {line}"
        );
    }
}

// ---------------------------------------------------------------------------
// Driving the endpoint
// ---------------------------------------------------------------------------

/// The one line `log` collected, parsed.
///
/// Every test here drives exactly one call, so more than one line is this
/// suite's own bug and is worth failing on rather than indexing past.
fn one(log: &Capture) -> Value {
    let lines = log.lines();
    assert_eq!(
        lines.len(),
        1,
        "one call should leave one line behind, got {lines:?}"
    );
    parse(&lines[0])
}

/// One line of the log, as the JSON it has to be.
///
/// Structured rather than prose is the whole point of the line, so a line
/// that does not parse is a failure of this suite and not of its caller.
fn parse(line: &str) -> Value {
    serde_json::from_str(line)
        .unwrap_or_else(|e| panic!("a log line should be one JSON object: {e}, got {line}"))
}

/// Call `tool` through the endpoint, with `log` behind every handler.
async fn call(log: &Capture, tool: &str, arguments: Value) -> Value {
    post(
        log,
        json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "tools/call",
            "params": { "name": tool, "arguments": arguments, "_meta": meta() },
        }),
    )
    .await
}

/// The per-request `_meta` a `2026-07-28` client attaches. See `tests/mcp.rs`.
fn meta() -> Value {
    json!({
        "io.modelcontextprotocol/protocolVersion": CURRENT,
        "io.modelcontextprotocol/clientCapabilities": {},
    })
}

/// A real request through the real router, with the fixture archives and the
/// capturing sink behind it.
///
/// Both arrive through the service factory `router_with` takes, which is the
/// path production takes to build a [`Ctx`] — a test that reached around it
/// would be testing wiring that does not exist.
async fn post(log: &Capture, body: Value) -> Value {
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

    let log = log.clone();
    let router = router::router_with(
        move || {
            Ok(Diffpack::with_ctx(
                Ctx::with_archive(Archive::fixture(FIXTURES)).logging_to(log.sink()),
            ))
        },
        Vec::new(),
    );

    let response = router
        .oneshot(request)
        .await
        .expect("the router should answer");
    let body = response
        .into_body()
        .collect()
        .await
        .expect("the body should read")
        .to_bytes();

    serde_json::from_slice(&body).expect("the answer should be JSON")
}
