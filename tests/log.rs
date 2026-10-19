use std::time::{Duration, Instant};

mod common;

use common::{Client, FIXTURES};
use diffpack_server::error::Failure;
use diffpack_server::log::{Capture, Line, Spent};
use diffpack_server::store::{DiffStore, Memory};
use diffpack_server::tools::{self, Ctx};
use serde_json::{json, Value};

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

#[tokio::test]
async fn a_call_that_failed_names_the_cause() {
    let log = Capture::new();

    call(
        &log,
        "list_package_files",
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

#[tokio::test]
async fn the_line_says_how_long_the_call_took() {
    let log = Capture::new();

    call(
        &log,
        "list_package_files",
        json!({ "registry": "npm", "package": "@types/node", "version": "20.1.0" }),
    )
    .await;

    let line = one(&log);
    let total = line["ms"]["total"]
        .as_f64()
        .unwrap_or_else(|| panic!("a call should be timed, got {line}"));

    assert!(
        total >= 0.0 && total.is_finite(),
        "a duration should be a real number of milliseconds, got {total}"
    );
}

#[tokio::test]
async fn the_fetch_is_timed_apart_from_the_rest_when_there_was_one() {
    let fetched = Capture::new();
    call(
        &fetched,
        "list_package_files",
        json!({ "registry": "npm", "package": "@types/node", "version": "20.1.0" }),
    )
    .await;

    let line = one(&fetched);
    let fetch = line["ms"]["fetch"]
        .as_f64()
        .unwrap_or_else(|| panic!("a call that read an archive spent time on it, got {line}"));
    let total = line["ms"]["total"].as_f64().expect("a call is timed");

    assert!(
        fetch <= total,
        "the fetch is part of the call, so it cannot outlast it: {fetch} of {total}"
    );

    let untouched = Capture::new();
    call(
        &untouched,
        "resolve_archive_url",
        json!({ "registry": "npm", "package": "@types/node", "version": "20.1.0" }),
    )
    .await;

    let line = one(&untouched);
    assert!(
        line["ms"]["fetch"].is_null(),
        "a tool that fetched nothing should report no fetch rather than zero, got {line}"
    );
}

#[tokio::test]
async fn a_call_that_only_read_a_catalogue_is_timed_as_a_wait_too() {
    let listed = Capture::new();
    call(
        &listed,
        "list_package_versions",
        json!({ "registry": "npm", "package": "zod" }),
    )
    .await;

    let line = one(&listed);
    let fetch = line["ms"]["fetch"]
        .as_f64()
        .unwrap_or_else(|| panic!("a call that read a catalogue waited on it, got {line}"));
    let total = line["ms"]["total"].as_f64().expect("a call is timed");

    assert!(
        fetch <= total,
        "the wait is part of the call, so it cannot outlast it: {fetch} of {total}"
    );
}

#[tokio::test]
async fn a_call_that_searched_a_registry_is_timed_as_a_wait_too() {
    let searched = Capture::new();
    call(
        &searched,
        "search_packages",
        json!({ "registry": "npm", "query": "zod" }),
    )
    .await;

    let line = one(&searched);
    let fetch = line["ms"]["fetch"]
        .as_f64()
        .unwrap_or_else(|| panic!("a call that searched a registry waited on it, got {line}"));
    let total = line["ms"]["total"].as_f64().expect("a call is timed");

    assert!(
        fetch <= total,
        "the wait is part of the call, so it cannot outlast it: {fetch} of {total}"
    );
}

#[tokio::test]
async fn a_tool_that_reads_two_archives_still_leaves_one_line() {
    let log = Capture::new();

    call(
        &log,
        "diff_package_versions",
        json!({
            "registry": "npm",
            "package": "zod",
            "from_version": "3.25.76",
            "to_version": "4.0.0",
        }),
    )
    .await;

    let line = one(&log);
    assert_eq!(
        line["result"], "ok",
        "the fixture set has this pair: {line}"
    );

    let fetch = line["ms"]["fetch"]
        .as_f64()
        .unwrap_or_else(|| panic!("two archives were read, got {line}"));
    let total = line["ms"]["total"].as_f64().expect("a call is timed");

    assert!(
        fetch <= total,
        "two fetches at once cost the window they span, which is inside the \
         call that spanned it: {fetch} of {total}"
    );
}

#[test]
fn concurrent_fetches_cost_the_window_they_span_and_not_their_sum() {
    let began = Instant::now();
    let at = |ms| began + Duration::from_millis(ms);

    let spent = Spent::starting_at(began);

    spent.fetching(at(0), at(50));
    spent.fetching(at(10), at(60));

    assert_eq!(
        spent.fetch(),
        Some(Duration::from_millis(60)),
        "the call waited from the first fetch starting to the last one ending"
    );
}

#[test]
fn a_request_that_fetched_nothing_reports_no_fetch() {
    assert_eq!(Spent::starting_at(Instant::now()).fetch(), None);
}

#[tokio::test]
async fn the_line_says_whether_the_answer_was_remembered() {
    let log = Capture::new();
    let store = Memory::new();

    call_storing(&log, &store, "diff_package_versions", diffable()).await;
    settles(&store).await;
    call_storing(&log, &store, "diff_package_versions", diffable()).await;

    let lines = log.lines();
    assert_eq!(
        lines.len(),
        2,
        "two calls should leave two lines, got {lines:?}"
    );

    assert_eq!(
        parse(&lines[0])["cache"],
        "miss",
        "the first call had nothing to read back: {}",
        lines[0]
    );
    assert_eq!(
        parse(&lines[1])["cache"],
        "hit",
        "the second asked for exactly what the first stored: {}",
        lines[1]
    );
}

#[tokio::test]
async fn a_deployment_with_no_store_says_so_rather_than_missing() {
    let log = Capture::new();

    call_without_a_store(&log, "diff_package_versions", diffable()).await;

    let line = one(&log);
    assert_eq!(
        line["cache"], "no_store",
        "there was no cache here to miss: {line}"
    );
}

#[tokio::test]
async fn a_tool_that_never_asks_the_store_says_nothing_about_it() {
    let log = Capture::new();

    call(
        &log,
        "list_package_files",
        json!({ "registry": "npm", "package": "@types/node", "version": "20.1.0" }),
    )
    .await;

    let line = one(&log);
    assert_eq!(
        line.get("cache"),
        None,
        "this tool has no cache to hit or miss: {line}"
    );
}

#[tokio::test]
async fn each_call_through_one_context_says_what_its_own_lookup_found() {
    let store = Memory::new();

    call_storing(&Capture::new(), &store, "diff_package_versions", diffable()).await;
    settles(&store).await;

    let log = Capture::new();
    let client = Client::over(
        Ctx::fixture(FIXTURES)
            .logging_to(log.sink())
            .storing_in(store.store()),
    );

    client.call("diff_package_versions", diffable()).await;
    client
        .call(
            "list_package_files",
            json!({ "registry": "npm", "package": "@types/node", "version": "20.1.0" }),
        )
        .await;

    let lines = log.lines();
    assert_eq!(
        lines.len(),
        2,
        "two calls should leave two lines, got {lines:?}"
    );

    assert_eq!(
        parse(&lines[0])["cache"],
        "hit",
        "the first call asked for exactly what was stored: {}",
        lines[0]
    );
    assert_eq!(
        parse(&lines[1]).get("cache"),
        None,
        "the second call never asked the store, whatever the first found: {}",
        lines[1]
    );
}

#[tokio::test]
async fn each_call_through_one_context_times_only_its_own_fetches() {
    let log = Capture::new();
    let client = Client::over(Ctx::fixture(FIXTURES).logging_to(log.sink()));
    let files = json!({ "registry": "npm", "package": "@types/node", "version": "20.1.0" });

    client.call("list_package_files", files.clone()).await;
    tokio::time::sleep(Duration::from_millis(50)).await;
    client.call("list_package_files", files).await;

    let lines = log.lines();
    assert_eq!(
        lines.len(),
        2,
        "two calls should leave two lines, got {lines:?}"
    );

    let second = parse(&lines[1]);
    let fetch = second["ms"]["fetch"]
        .as_f64()
        .unwrap_or_else(|| panic!("the second call downloaded an archive: {second}"));
    let total = second["ms"]["total"]
        .as_f64()
        .unwrap_or_else(|| panic!("every line says how long its call took: {second}"));

    assert!(
        fetch <= total,
        "the second call cannot have waited longer than it ran: {second}"
    );
}

#[tokio::test]
async fn a_resource_read_leaves_no_line_until_26_says_what_it_holds() {
    let log = Capture::new();
    let client = Client::over(Ctx::fixture(FIXTURES).logging_to(log.sink()));

    let summary = client.call("diff_package_versions", diffable()).await;
    let handle = summary["structuredContent"]["handle"]
        .as_str()
        .unwrap_or_else(|| panic!("the summary carries a handle, got {summary}"));

    let read = client
        .post(json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "resources/read",
            "params": { "uri": format!("diffpack://diff/{handle}") },
        }))
        .await;
    assert!(
        read["result"]["contents"][0]["text"].is_string(),
        "the read should have answered, got {read}"
    );

    let lines = log.lines();
    assert_eq!(
        lines.len(),
        1,
        "only the tool call should have left a line, got {lines:?}"
    );
    assert_eq!(
        parse(&lines[0])["tool"],
        "diff_package_versions",
        "the one line is the tool call's: {}",
        lines[0]
    );
}

#[tokio::test]
async fn a_call_that_panics_still_leaves_its_line() {
    let log = Capture::new();
    let ctx = Ctx::fixture(FIXTURES).logging_to(log.sink());

    let answer = tools::run(
        &ctx,
        Line::new("get_diff_tree"),
        async |_| -> Result<(), Failure> {
            panic!("deliberately panicking, so that the dispatch has something to catch")
        },
    )
    .await;

    let line = one(&log);
    assert_eq!(
        line["tool"], "get_diff_tree",
        "the call that crashed is the one an incident asks about: {line}"
    );
    assert_eq!(
        line["result"], "panicked",
        "a panic is its own cause, not an internal failure a handler returned: {line}"
    );

    let refused = answer
        .expect_err("a call that panicked has no answer to give")
        .refuse();
    assert_eq!(
        refused.code.0, -32603,
        "a panic is JSON-RPC's own internal error, as it was before the dispatch caught it"
    );
}

#[tokio::test]
async fn no_line_carries_a_credential_or_a_signed_url() {
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

#[tokio::test]
async fn a_credential_in_a_name_reaches_no_line_either() {
    let secret = "https://blob.vercel-storage.com/diffs/abc?token=s3cr3t&expires=1";

    let named_argument = Capture::new();
    call(
        &named_argument,
        "list_package_files",
        json!({
            "registry": "npm",
            "package": "@types/node",
            "version": "20.1.0",
            secret: "1",
        }),
    )
    .await;

    let line = one(&named_argument).to_string();
    assert!(
        !line.contains(secret),
        "an argument's name is as much the caller's as its value: {line}"
    );

    let named_tool = Capture::new();
    call(&named_tool, secret, json!({})).await;

    let line = one(&named_tool).to_string();
    assert!(
        !line.contains(secret),
        "a name no tool answers to is still written down: {line}"
    );
}

fn one(log: &Capture) -> Value {
    let lines = log.lines();
    assert_eq!(
        lines.len(),
        1,
        "one call should leave one line behind, got {lines:?}"
    );
    parse(&lines[0])
}

fn parse(line: &str) -> Value {
    serde_json::from_str(line)
        .unwrap_or_else(|e| panic!("a log line should be one JSON object: {e}, got {line}"))
}

#[derive(Clone)]
enum Store {
    Fresh,

    Held(Memory),

    Absent,
}

async fn call(log: &Capture, tool: &str, arguments: Value) -> Value {
    calling(log, Store::Fresh, tool, arguments).await
}

async fn call_storing(log: &Capture, store: &Memory, tool: &str, arguments: Value) -> Value {
    calling(log, Store::Held(store.clone()), tool, arguments).await
}

async fn call_without_a_store(log: &Capture, tool: &str, arguments: Value) -> Value {
    calling(log, Store::Absent, tool, arguments).await
}

async fn calling(log: &Capture, store: Store, tool: &str, arguments: Value) -> Value {
    let ctx = Ctx::fixture(FIXTURES).logging_to(log.sink());
    let ctx = match store {
        Store::Fresh => ctx,
        Store::Held(memory) => ctx.storing_in(memory.store()),
        Store::Absent => ctx.storing_in(DiffStore::unavailable()),
    };

    Client::over(ctx)
        .post(json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "tools/call",
            "params": { "name": tool, "arguments": arguments },
        }))
        .await
}

fn diffable() -> Value {
    json!({
        "registry": "npm",
        "package": "diffable",
        "from_version": "1.0.0",
        "to_version": "2.0.0",
    })
}

async fn settles(store: &Memory) {
    for _ in 0..400 {
        if store.written().len() >= 2 {
            return;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }

    panic!(
        "the entry should be written by now, the store holds {:?}",
        store.written()
    );
}
