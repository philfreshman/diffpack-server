//! How failure reaches a client, settled once.
//!
//! MCP has two channels and conflating them is the usual mistake. A protocol
//! error is a JSON-RPC error: the method does not exist, the parameters do
//! not validate. The model never sees it — a client renders it opaquely, if
//! at all. A tool error is a *successful* JSON-RPC response carrying
//! `isError: true`, and the model does see it, and can act on it.
//!
//! "crates.io has no version 9.9.9 of serde" is something an agent can
//! recover from by asking for a version that exists, so it belongs in the
//! second channel. Putting it in the first would hide it from the only reader
//! who could do anything about it.
//!
//! These tests fix that split, and fix the shape of the messages, before the
//! nineteen tools of phases 3 and 4 each invent an answer of their own.

use std::time::Duration;

mod common;

use axum::http::StatusCode;
use common::Client;
use diffpack_server::error::{self, Failure};
use diffpack_server::router;
use serde_json::{json, Value};

// ---------------------------------------------------------------------------
// The protocol channel
// ---------------------------------------------------------------------------

/// A method this server does not implement is the client's mistake, not
/// something a model should be shown. The transport spec asks for `404` on
/// top of the JSON-RPC code so that a client can tell "wrong endpoint" from
/// "wrong method" without parsing a body.
#[tokio::test]
async fn an_unknown_method_is_not_found_and_minus_32601() {
    let answer = Client::fixture()
        .respond(json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "diffpack/no-such-method",
            "params": {},
        }))
        .await;

    assert_eq!(answer.status, StatusCode::NOT_FOUND);
    assert_eq!(answer.json()["error"]["code"], -32601);
}

/// Naming a tool that does not exist is the same kind of mistake: there is no
/// tool to have failed, so there is no tool error to report.
///
/// The call is well formed down to the SEP-2243 `Mcp-Name` header, so the
/// name reaches the dispatch in `src/tools` and is refused there. Without the
/// header the transport would refuse it first, with `-32020`, and this test
/// would pass without ever reaching the code it is about.
#[tokio::test]
async fn calling_a_tool_that_does_not_exist_is_a_protocol_error() {
    let answer = Client::fixture()
        .post(json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "tools/call",
            "params": { "name": "no_such_tool", "arguments": {} },
        }))
        .await;

    assert_eq!(
        answer["error"]["code"], -32602,
        "a name that does not resolve is a parameter that does not validate, got {answer}"
    );
    assert!(
        answer["result"]["isError"].is_null(),
        "a tool that does not exist must not be reported as a tool that failed"
    );
}

/// Arguments that do not validate are a protocol error — `-32602` — and never
/// an `isError` result. A model cannot fix a call it did not get to make, and
/// a client that saw `isError` would show the model a failure it could not
/// act on instead of correcting the call it sent.
#[test]
fn invalid_params_is_a_protocol_error() {
    let error = Failure::InvalidParams {
        message: "`from` is required".to_owned(),
    }
    .respond()
    .expect_err("invalid parameters belong in the protocol channel");

    assert_eq!(error.code.0, -32602);
}

/// Resource-not-found moved from `-32002` to `-32602` in `2026-07-28`: a URI
/// that does not resolve is now a parameter that does not validate, which it
/// always was.
#[test]
fn a_resource_that_does_not_resolve_is_invalid_params() {
    let error = Failure::NoSuchResource {
        uri: "diff://nothing".to_owned(),
    }
    .respond()
    .expect_err("a missing resource belongs in the protocol channel");

    assert_eq!(error.code.0, -32602);
}

/// Our own failures get a code from the range an implementation is allowed to
/// use. `-32020`..`-32099` is reserved for the specification — rmcp already
/// has three codes there — so allocating into it would collide with a
/// revision nobody has written yet.
#[test]
fn our_own_codes_stay_inside_the_implementation_range() {
    let error = Failure::Internal {
        doing: "reading the cache",
    }
    .respond()
    .expect_err("an internal failure belongs in the protocol channel");

    assert!(
        (-32019..=-32000).contains(&error.code.0),
        "{} is outside the -32000..-32019 an implementation may allocate",
        error.code.0
    );
}

// ---------------------------------------------------------------------------
// The tool channel
// ---------------------------------------------------------------------------

/// The message a model reads has to name what was asked for, what happened,
/// and what would work instead. A `404` tells it none of those, and the
/// recovery — ask for a version that exists — is only available if the
/// versions that exist are in the message.
#[test]
fn a_missing_version_names_the_package_the_version_and_a_way_forward() {
    let result = Failure::NoSuchVersion {
        registry: "npm".to_owned(),
        package: "zod".to_owned(),
        version: "9.9.9".to_owned(),
        known: vec!["4.0.0".to_owned(), "3.25.76".to_owned()],
    }
    .respond()
    .expect("a version that does not exist is something a model can recover from");

    assert_eq!(result.is_error, Some(true));

    let text = text_of(&result);
    for expected in ["zod", "9.9.9", "npm", "4.0.0", "3.25.76"] {
        assert!(
            text.contains(expected),
            "the message should name {expected}, got: {text}"
        );
    }
}

/// A package that does not exist at all is a different problem with a
/// different remedy — check the name, or the registry — so it must not
/// collapse into the message above.
#[test]
fn a_missing_package_is_not_a_missing_version() {
    let missing_package = message_of(Failure::NoSuchPackage {
        registry: "crates.io".to_owned(),
        package: "zdo".to_owned(),
    });

    assert!(missing_package.contains("zdo") && missing_package.contains("crates.io"));
    assert!(
        !missing_package.contains("version"),
        "a package that does not exist has no version to talk about: {missing_package}"
    );
}

/// Eight upstream causes, eight remedies: wait and retry, slow down, check
/// the name, report a broken archive, ask for something smaller, a registry
/// this server could not reach at all — the one with no status behind it —
/// and the two that are about a package's versions rather than about one
/// version's files: a document that would not read, and one this server would
/// not hold. A single "registry error" string would leave a model guessing
/// which one it is looking at, so every one of them has to read differently.
///
/// The pairs are what this is really guarding. `MalformedArchive` and
/// `UnreadableVersions` both end in a reason, and `TooLarge` and
/// `VersionsTooLarge` both end in a size and a cap; either pair could
/// collapse into one sentence without the compiler noticing.
#[test]
fn every_upstream_cause_reads_differently() {
    let messages = [
        message_of(Failure::NoSuchPackage {
            registry: "npm".to_owned(),
            package: "zod".to_owned(),
        }),
        message_of(Failure::RateLimited {
            registry: "npm".to_owned(),
            retry_after: Some(Duration::from_secs(30)),
        }),
        message_of(Failure::TimedOut {
            registry: "npm".to_owned(),
            waited: error::UPSTREAM_TIMEOUT,
        }),
        message_of(Failure::MalformedArchive {
            package: "zod".to_owned(),
            version: "4.0.0".to_owned(),
            reason: "unexpected end of archive".to_owned(),
        }),
        message_of(Failure::TooLarge {
            package: "zod".to_owned(),
            version: "4.0.0".to_owned(),
            bytes: 300_000_000,
            limit: 256 * 1024 * 1024,
        }),
        message_of(Failure::Unreachable {
            registry: "npm".to_owned(),
        }),
        message_of(Failure::UnreadableVersions {
            registry: "npm".to_owned(),
            package: "zod".to_owned(),
            reason: "what the registry served is not text".to_owned(),
        }),
        message_of(Failure::VersionsTooLarge {
            registry: "npm".to_owned(),
            package: "zod".to_owned(),
            bytes: 40_000_000,
            limit: 32 * 1024 * 1024,
        }),
        message_of(Failure::UnreadableSearch {
            registry: "npm".to_owned(),
            reason: "what the registry served is not text".to_owned(),
        }),
        message_of(Failure::SearchTooLarge {
            registry: "PyPI".to_owned(),
            bytes: 70_000_000,
            limit: 64 * 1024 * 1024,
        }),
    ];

    let mut seen: Vec<&str> = messages.iter().map(String::as_str).collect();
    seen.sort_unstable();
    let before = seen.len();
    seen.dedup();

    assert_eq!(
        seen.len(),
        before,
        "two upstream causes share a message: {messages:#?}"
    );
}

/// Every Failure names itself differently in a line, so that a count by cause
/// is a count of causes.
///
/// `Failure::kind` is exhaustive, which stops a new variant reaching a line
/// without a name of its own. It does not stop one reaching a line under a
/// name that is already taken — a word copied from the arm above compiles,
/// and the two failures it now covers are one bucket in every rate built on
/// these lines. Nothing about them would look wrong; there would simply be a
/// cause nobody could count.
///
/// Enumerated by hand for the reason the sibling test above gives: a list
/// derived from the enum would be derived from the thing under test.
#[test]
fn every_cause_a_line_can_carry_names_one_failure() {
    let causes = [
        Failure::NoSuchPackage {
            registry: "npm".to_owned(),
            package: "zod".to_owned(),
        }
        .kind(),
        Failure::NoSuchVersion {
            registry: "npm".to_owned(),
            package: "zod".to_owned(),
            version: "99.9.9".to_owned(),
            known: Vec::new(),
        }
        .kind(),
        Failure::RateLimited {
            registry: "npm".to_owned(),
            retry_after: None,
        }
        .kind(),
        Failure::TimedOut {
            registry: "npm".to_owned(),
            waited: error::UPSTREAM_TIMEOUT,
        }
        .kind(),
        Failure::Unreachable {
            registry: "npm".to_owned(),
        }
        .kind(),
        Failure::Unavailable {
            registry: "npm".to_owned(),
            status: 503,
        }
        .kind(),
        Failure::MalformedArchive {
            package: "zod".to_owned(),
            version: "4.0.0".to_owned(),
            reason: "unexpected end of archive".to_owned(),
        }
        .kind(),
        Failure::TooLarge {
            package: "zod".to_owned(),
            version: "4.0.0".to_owned(),
            bytes: 300_000_000,
            limit: 128 * 1024 * 1024,
        }
        .kind(),
        Failure::UnreadableVersions {
            registry: "npm".to_owned(),
            package: "zod".to_owned(),
            reason: "what the registry served is not text".to_owned(),
        }
        .kind(),
        Failure::VersionsTooLarge {
            registry: "npm".to_owned(),
            package: "zod".to_owned(),
            bytes: 300_000_000,
            limit: 8 * 1024 * 1024,
        }
        .kind(),
        Failure::UnreadableSearch {
            registry: "npm".to_owned(),
            reason: "what the registry served is not text".to_owned(),
        }
        .kind(),
        Failure::SearchTooLarge {
            registry: "PyPI".to_owned(),
            bytes: 300_000_000,
            limit: 64 * 1024 * 1024,
        }
        .kind(),
        Failure::NoSuchFile {
            package: "zod".to_owned(),
            version: "4.0.0".to_owned(),
            path: "src/gone.ts".to_owned(),
        }
        .kind(),
        Failure::PathIsDirectory {
            package: "zod".to_owned(),
            version: "4.0.0".to_owned(),
            path: "src".to_owned(),
        }
        .kind(),
        Failure::ItemTooLarge {
            position: 3,
            bytes: 9_000_000,
            ceiling: 4_500_000,
            resume: "the cursor".to_owned(),
        }
        .kind(),
        Failure::UnresolvableArchiveUrl {
            registry: "pypi".to_owned(),
            resolvable: Vec::new(),
        }
        .kind(),
        Failure::InvalidParams {
            message: "`from` is required".to_owned(),
        }
        .kind(),
        Failure::NoSuchTool {
            name: "diff_everything".to_owned(),
        }
        .kind(),
        Failure::NoSuchResource {
            uri: "diffpack://nothing".to_owned(),
        }
        .kind(),
        Failure::Internal {
            doing: "answering a tool call",
        }
        .kind(),
    ];

    let mut seen = causes.to_vec();
    seen.sort_unstable();
    let before = seen.len();
    seen.dedup();

    assert_eq!(
        seen.len(),
        before,
        "two failures share a cause, so neither can be counted: {causes:#?}"
    );
}

/// A timeout and a rate limit are worth retrying; a package that does not
/// exist is not. A model that cannot tell them apart either gives up on
/// something transient or hammers something permanent.
#[test]
fn the_transient_failures_say_so_and_the_permanent_ones_do_not() {
    for transient in [
        Failure::TimedOut {
            registry: "PyPI".to_owned(),
            waited: error::UPSTREAM_TIMEOUT,
        },
        Failure::RateLimited {
            registry: "PyPI".to_owned(),
            retry_after: None,
        },
        // A name that did not resolve, a refused connection, a TLS handshake
        // that failed: the registry never answered, so there is no status to
        // report and nothing for the caller to have done differently.
        Failure::Unreachable {
            registry: "PyPI".to_owned(),
        },
    ] {
        let message = message_of(transient);
        assert!(
            message.contains("again"),
            "a transient failure should invite a retry: {message}"
        );
    }

    let permanent = message_of(Failure::NoSuchPackage {
        registry: "PyPI".to_owned(),
        package: "nope".to_owned(),
    });
    assert!(
        !permanent.contains("again"),
        "asking again will not make the package exist: {permanent}"
    );
}

/// A truncated or corrupt archive is the registry's problem, not the
/// caller's, and it is not a panic. The engine's extractor returns an error
/// for rubbish bytes (`tests/engine.rs`); this is where that error becomes
/// something a model can read.
#[test]
fn a_malformed_archive_is_a_tool_error_naming_the_archive() {
    let result = Failure::MalformedArchive {
        package: "zod".to_owned(),
        version: "4.0.0".to_owned(),
        reason: "unexpected end of archive".to_owned(),
    }
    .respond()
    .expect("a broken archive is something a model should be told about");

    assert_eq!(result.is_error, Some(true));

    let text = text_of(&result);
    assert!(text.contains("zod") && text.contains("4.0.0"));
}

// ---------------------------------------------------------------------------
// Budgets
// ---------------------------------------------------------------------------

/// Every outbound request is bounded, and bounded well inside the function's
/// own limit. A request left to run until Vercel kills the function produces
/// no response at all: the client sees a dead connection instead of a message
/// saying which registry was slow.
///
/// The margin is generous on purpose — a tool may make more than one outbound
/// request, and the diff still has to be computed and written after the last
/// of them.
#[test]
fn the_upstream_budget_leaves_the_function_time_to_answer() {
    let manifest: Value =
        serde_json::from_str(include_str!("../vercel.json")).expect("vercel.json should parse");

    let max_duration = manifest["functions"]["api/mcp.rs"]["maxDuration"]
        .as_u64()
        .expect("vercel.json should give the handler a maxDuration");

    assert!(
        error::UPSTREAM_TIMEOUT.as_secs() * 4 < max_duration,
        "an upstream budget of {}s leaves too little of the function's {max_duration}s \
         to answer in",
        error::UPSTREAM_TIMEOUT.as_secs()
    );
}

/// A registry that never answers fails inside the budget rather than hanging
/// the function to its limit, and fails as a timeout rather than as something
/// generic.
///
/// Time is paused, so this asserts against the real budget without waiting
/// for it.
#[tokio::test(start_paused = true)]
async fn a_request_that_never_answers_times_out_inside_the_budget() {
    let never = std::future::pending::<()>();

    let failure = error::within_budget("crates.io", never)
        .await
        .expect_err("a future that never completes should not be waited on forever");

    assert!(
        matches!(failure, Failure::TimedOut { .. }),
        "expected a timeout, got {failure:?}"
    );
    assert!(message_of(failure).contains("crates.io"));
}

// ---------------------------------------------------------------------------
// Redaction
// ---------------------------------------------------------------------------

/// Nothing a client sees may carry a credential, a signed URL or a path
/// inside the function.
///
/// The structural defence is that messages are built from fields this crate
/// chose, never from an upstream error's own `Display`. The two fields that
/// do carry free text — an archive's reason, an internal failure's context —
/// go through [`error::redact`] first, and this is what it has to catch.
///
/// #20 repeats this against a failure constructed inside the real Blob
/// client; until that client exists, the token shape is the thing under test.
#[test]
fn credentials_and_internal_paths_never_reach_the_client() {
    // The token is assembled rather than written out, and deliberately so: a
    // literal of this shape is indistinguishable from a real credential to a
    // secret scanner, which flags it on every pull request that touches this
    // file. A check that cries wolf is one people learn to click past, and
    // this repository's scanner is worth keeping useful. What `redact` sees
    // is the assembled string, so the thing under test is unchanged.
    //
    // Do not "tidy" this back into one literal.
    let blob_token = ["vercel", "blob", "rw", "A1b2C3d4E5f6G7h8i9J0kL1mN2oP3qR4"].join("_");

    let secrets = [
        blob_token.as_str(),
        "https://blob.vercel-storage.com/diffs/abc?token=s3cr3t&expires=1",
        "/var/task/diffpack-server/src/cache.rs",
    ];

    for secret in secrets {
        let redacted = error::redact(&format!("while reading {secret} the store said no"));
        assert!(
            !redacted.contains(secret),
            "`{secret}` survived redaction: {redacted}"
        );
    }
}

/// Redaction that ate the message would be its own failure: an error nobody
/// can read is as useless as one that leaks.
#[test]
fn redaction_leaves_the_readable_part_alone() {
    let readable = "the archive for zod 4.0.0 ended after 12 bytes";
    assert_eq!(error::redact(readable), readable);
}

/// The rule holds through the type, not only through `redact`: a malformed
/// archive's `reason` is free text from a library, so it is the field most
/// likely to carry a path one day.
#[test]
fn free_text_fields_are_redacted_on_the_way_out() {
    let message = message_of(Failure::MalformedArchive {
        package: "zod".to_owned(),
        version: "4.0.0".to_owned(),
        reason: "failed opening /var/task/tmp/zod.tgz".to_owned(),
    });

    assert!(
        !message.contains("/var/task"),
        "an internal path reached the client: {message}"
    );
}

// ---------------------------------------------------------------------------
// Panics
// ---------------------------------------------------------------------------

/// A panic must become an answer. Without this the panic unwinds into the
/// connection task `vercel_runtime` spawns and the client sees the connection
/// drop — no status, no body, nothing to log against the request that caused
/// it. A JSON-RPC internal error is at least something a client can render
/// and a caller can report.
#[tokio::test]
async fn a_panicking_handler_answers_with_a_json_rpc_error() {
    let answer = Client::routed(|| router::router_with(|| Ok(Panicking), vec![]))
        .respond(json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "tools/list",
            "params": {},
        }))
        .await;
    let body = answer.json();

    assert_ne!(
        answer.status,
        StatusCode::INTERNAL_SERVER_ERROR,
        "a panic should not reach the client as a bare 500"
    );
    assert_eq!(
        body["error"]["code"], -32603,
        "a panic is an internal error, got {body}"
    );
    assert!(
        !body["error"]["message"]
            .as_str()
            .unwrap_or_default()
            .is_empty(),
        "the error should say something"
    );
}

/// A handler that panics on every request, so the layer above it has
/// something to catch.
#[derive(Debug, Clone)]
struct Panicking;

impl rmcp::ServerHandler for Panicking {
    fn get_info(&self) -> rmcp::model::ServerConfig {
        rmcp::model::ServerConfig::new(
            rmcp::model::ServerCapabilities::builder()
                .enable_tools()
                .build(),
        )
    }

    async fn list_tools(
        &self,
        _request: Option<rmcp::model::PaginatedRequestParams>,
        _context: rmcp::service::RequestContext<rmcp::RoleServer>,
    ) -> Result<rmcp::model::ListToolsResult, rmcp::ErrorData> {
        panic!("deliberately panicking, so that the layer above has something to catch");
    }
}

// ---------------------------------------------------------------------------
// Delegation
// ---------------------------------------------------------------------------

/// Every request reaches the handler that was wrapped, including the methods
/// this crate has not implemented yet.
///
/// `Guarded` is a wrapper, and a wrapper that implements a trait method
/// silently replaces the inner handler's answer with its own. For a method
/// like `resources/list` the trait's default is an empty list, so an
/// overlooked method does not fail — it succeeds, with the wrong answer, and
/// nothing anywhere says so. `Failure::NoSuchResource` exists, so resources
/// are coming; this is the test that notices if they arrive behind a wrapper
/// that swallows them.
///
/// `resources/list` is the probe rather than the subject: what is under test
/// is that `Guarded` forwards, and any method it did not override would do.
#[tokio::test]
async fn a_method_this_crate_has_not_implemented_still_reaches_the_handler() {
    let answer = Client::routed(|| router::router_with(|| Ok(WithResources), vec![]))
        .respond(json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "resources/list",
            "params": {},
        }))
        .await;
    let body = answer.json();

    assert_eq!(answer.status, StatusCode::OK, "got {body}");
    assert_eq!(
        body["result"]["resources"][0]["uri"],
        json!(WITH_RESOURCES_URI),
        "the wrapper answered instead of the handler it wraps: {body}"
    );
}

const WITH_RESOURCES_URI: &str = "diff://npm/zod/3.25.76...4.0.0";

/// A handler implementing a method `Guarded` does not itself override, so
/// that a reply carrying this resource proves the call was forwarded.
#[derive(Debug, Clone)]
struct WithResources;

impl rmcp::ServerHandler for WithResources {
    fn get_info(&self) -> rmcp::model::ServerConfig {
        rmcp::model::ServerConfig::new(
            rmcp::model::ServerCapabilities::builder()
                .enable_resources()
                .build(),
        )
    }

    async fn list_resources(
        &self,
        _request: Option<rmcp::model::PaginatedRequestParams>,
        _context: rmcp::service::RequestContext<rmcp::RoleServer>,
    ) -> Result<rmcp::model::ListResourcesResult, rmcp::ErrorData> {
        Ok(rmcp::model::ListResourcesResult::with_all_items(vec![
            rmcp::model::Resource::new(WITH_RESOURCES_URI, "zod 3.25.76 to 4.0.0"),
        ]))
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// The text a model would read for `failure`, whichever channel it takes.
fn message_of(failure: Failure) -> String {
    match failure.respond() {
        Ok(result) => text_of(&result),
        Err(error) => error.message.into_owned(),
    }
}

fn text_of(result: &rmcp::model::CallToolResult) -> String {
    result
        .content
        .iter()
        .filter_map(|block| block.as_text().map(|text| text.text.clone()))
        .collect::<Vec<_>>()
        .join("\n")
}
