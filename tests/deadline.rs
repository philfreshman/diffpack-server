use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

mod common;

use common::FIXTURES;
use diffpack_server::archive::Archive;
use diffpack_server::error::Failure;
use diffpack_server::log::{Capture, Line};
use diffpack_server::registry::Registry;
use diffpack_server::tools::{self, Ctx};
use serde_json::Value;

const BUDGET: Duration = Duration::from_millis(50);

const LONGER_THAN_THE_BUDGET: Duration = Duration::from_secs(1);

#[tokio::test]
async fn a_call_that_outlasts_its_budget_is_answered_rather_than_waited_on() {
    let log = Capture::new();
    let ctx = Ctx::fixture(FIXTURES)
        .logging_to(log.sink())
        .answering_within(BUDGET);
    let started = Instant::now();

    let answer = tools::run(
        &ctx,
        Line::calling("diff_package_versions"),
        async |_| -> Result<(), Failure> {
            tokio::time::sleep(LONGER_THAN_THE_BUDGET).await;
            Ok(())
        },
    )
    .await;

    assert!(
        started.elapsed() < LONGER_THAN_THE_BUDGET,
        "the call should have been answered when its budget ran out, after {:?}",
        started.elapsed()
    );

    let failure = answer.expect_err("a call out of time has no answer to give");
    assert_eq!(failure.kind(), "out_of_time");

    let lines = log.lines();
    let line: Value = serde_json::from_str(&lines[0]).expect("a line is JSON");
    assert_eq!(
        line["result"], "out_of_time",
        "the line says the call ran out of time rather than failed: {line}"
    );
}

#[tokio::test]
async fn work_that_holds_a_thread_is_still_answered_when_the_budget_runs_out() {
    let log = Capture::new();
    let ctx = Ctx::fixture(FIXTURES)
        .logging_to(log.sink())
        .answering_within(BUDGET);
    let started = Instant::now();

    let answer = tools::run(&ctx, Line::calling("diff_package_versions"), async |call| {
        call.compute("comparing two versions", || {
            std::thread::sleep(LONGER_THAN_THE_BUDGET);
        })
        .await
    })
    .await;

    assert!(
        started.elapsed() < LONGER_THAN_THE_BUDGET,
        "work that never yields should not hold the answer back, answered after {:?}",
        started.elapsed()
    );
    assert_eq!(
        answer
            .expect_err("a call out of time has no answer to give")
            .kind(),
        "out_of_time"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn extracting_an_archive_lets_the_runtime_get_on_with_other_work() {
    let other_work_ran = Arc::new(AtomicBool::new(false));
    tokio::spawn({
        let other_work_ran = Arc::clone(&other_work_ran);
        async move { other_work_ran.store(true, Ordering::SeqCst) }
    });

    Archive::fixture(format!("{FIXTURES}/archives"))
        .fetch(Registry::Npm, "diffable", "1.0.0")
        .await
        .expect("the fixture set has this archive");

    assert!(
        other_work_ran.load(Ordering::SeqCst),
        "on one thread, a task spawned before the fetch runs during it only if the \
         extraction gave the thread up, which is what lets a call's budget run out \
         while an archive is still being read"
    );
}
