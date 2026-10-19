use std::time::Duration;

mod common;

use axum::http::StatusCode;
use common::Client;
use diffpack_server::error::{self, Failure};
use diffpack_server::router;
use serde_json::{json, Value};

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

#[test]
fn invalid_params_is_a_protocol_error() {
    let error = Failure::InvalidParams {
        message: "`from` is required".to_owned(),
    }
    .respond()
    .expect_err("invalid parameters belong in the protocol channel");

    assert_eq!(error.code.0, -32602);
}

#[test]
fn a_resource_that_does_not_resolve_is_invalid_params() {
    let error = Failure::NoSuchResource {
        uri: "diff://nothing".to_owned(),
    }
    .respond()
    .expect_err("a missing resource belongs in the protocol channel");

    assert_eq!(error.code.0, -32602);
}

#[test]
fn a_prompt_that_does_not_exist_is_invalid_params() {
    let error = Failure::NoSuchPrompt {
        name: "review_everything".to_owned(),
    }
    .refuse();

    assert_eq!(error.code.0, -32602);
}

#[test]
fn our_own_codes_stay_inside_the_implementation_range() {
    for failure in [
        Failure::Internal {
            doing: "reading the cache",
        },
        Failure::NoSuchPackage {
            registry: "npm".to_owned(),
            package: "nope".to_owned(),
        },
        Failure::Busy {
            waited: error::UPSTREAM_TIMEOUT,
        },
        Failure::OutOfTime {
            budget: error::CALL_BUDGET,
        },
        Failure::TooLarge {
            package: "zod".to_owned(),
            version: "4.0.0".to_owned(),
            bytes: 90_000_000,
            limit: 40_000_000,
        },
    ] {
        let cause = failure.kind();
        let code = failure.refuse().code.0;

        assert!(
            (-32019..=-32000).contains(&code),
            "`{cause}` is {code}, outside the -32000..-32019 an implementation \
             may allocate"
        );
        assert_ne!(
            code, -32002,
            "`{cause}` took RESOURCE_NOT_FOUND, which a client below \
             2026-07-28 reads as a URI this server does not serve"
        );
    }
}

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
    for expected in [
        "zod",
        "9.9.9",
        "npm",
        "4.0.0",
        "3.25.76",
        "`list_package_versions`",
    ] {
        assert!(
            text.contains(expected),
            "the message should name {expected}, got: {text}"
        );
    }
}

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
        Failure::NoSuchPrompt {
            name: "review_everything".to_owned(),
        }
        .kind(),
        Failure::Internal {
            doing: "answering a tool call",
        }
        .kind(),
        Failure::Panicked {
            doing: "calling a tool",
        }
        .kind(),
        Failure::OutOfTime {
            budget: error::CALL_BUDGET,
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
        Failure::Unreachable {
            registry: "PyPI".to_owned(),
        },
        Failure::OutOfTime {
            budget: error::CALL_BUDGET,
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

#[test]
fn the_call_budget_leaves_the_function_time_to_answer() {
    let manifest: Value =
        serde_json::from_str(include_str!("../vercel.json")).expect("vercel.json should parse");

    let max_duration = manifest["functions"]["api/mcp.rs"]["maxDuration"]
        .as_u64()
        .expect("vercel.json should give the handler a maxDuration");

    assert!(
        (error::CALL_BUDGET + error::UPSTREAM_TIMEOUT).as_secs() <= max_duration,
        "a call budget of {}s leaves too little of the function's {max_duration}s to \
         answer in",
        error::CALL_BUDGET.as_secs()
    );
}

#[test]
fn the_call_budget_outlasts_the_longest_wait_on_a_registry() {
    let a_listing_then_its_archive = 2;
    let a_slot_then_a_request = 2;
    let longest_wait = error::UPSTREAM_TIMEOUT * a_listing_then_its_archive * a_slot_then_a_request;

    assert!(
        error::CALL_BUDGET > longest_wait,
        "a registry that is slow should be answered as slow, not as a call out of \
         time: {:?} against {longest_wait:?}",
        error::CALL_BUDGET
    );
}

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

#[test]
fn credentials_and_internal_paths_never_reach_the_client() {
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

#[test]
fn redaction_leaves_the_readable_part_alone() {
    let readable = "the archive for zod 4.0.0 ended after 12 bytes";
    assert_eq!(error::redact(readable), readable);
}

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
