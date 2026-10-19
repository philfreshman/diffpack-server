use std::time::{Duration, Instant};

mod common;

use common::{Client, FIXTURES};
use diffpack_server::log::Capture;
use diffpack_server::store::{DiffStore, Memory, Operation};
use diffpack_server::tools::Ctx;
use serde_json::{json, Value};

const TOOL: &str = "diff_package_versions";

const ONE_SIDED: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/fixtures/one-sided");

const NOT_PUBLISHED: i64 = -32001;

fn diffable() -> Value {
    json!({
        "registry": "npm",
        "package": "diffable",
        "from_version": "1.0.0",
        "to_version": "2.0.0",
    })
}

fn unchanged() -> Value {
    json!({
        "registry": "npm",
        "package": "diffable",
        "from_version": "1.0.0",
        "to_version": "1.0.0",
    })
}

#[tokio::test]
async fn the_same_diff_asked_for_twice_is_remembered_the_second_time() {
    let store = Memory::new();

    let first = call(|| store.store(), diffable()).await;
    assert_eq!(
        first["structuredContent"]["cached"],
        json!(false),
        "nothing had been diffed yet, got {first}"
    );
    written_at_least(&store, 2);

    let second = call(|| store.store(), diffable()).await;
    assert_eq!(
        second["structuredContent"]["cached"],
        json!(true),
        "the same comparison a second time is the one that was written, got \
         {second}"
    );
}

#[tokio::test]
async fn a_remembered_answer_is_the_one_that_was_computed() {
    let store = Memory::new();

    let computed = call(|| store.store(), diffable()).await;
    written_at_least(&store, 2);

    let remembered = call(|| store.store(), diffable()).await;

    assert_eq!(
        but_for_cached(remembered),
        but_for_cached(computed),
        "everything but `cached` is the same answer, the text block included"
    );
}

#[tokio::test]
async fn a_remembered_diff_is_two_blobs_under_the_name_the_comparison_has() {
    let store = Memory::new();

    let answer = call(|| store.store(), diffable()).await;
    written_at_least(&store, 2);

    let diff_id = answer["structuredContent"]["diff_id"]
        .as_str()
        .unwrap_or_else(|| panic!("the answer names the comparison, got {answer}"));

    assert_eq!(
        store.written(),
        vec![
            format!("diffs/v1/{diff_id}/meta.json"),
            format!("diffs/v1/{diff_id}/patches.json"),
        ],
        "one comparison is these two blobs and no others"
    );
}

#[tokio::test]
async fn every_changed_file_is_remembered_with_its_patch_already_rendered() {
    let store = Memory::new();

    let answer = call(|| store.store(), diffable()).await;
    written_at_least(&store, 2);

    assert_eq!(
        blob(&store, &patches_of(&answer)),
        json!({
            "src/added.js": {
                "data": "--- /dev/null\n+++ to/src/added.js\n+ export const fresh = true;\n+ ",
                "is_diff": true,
            },
            "src/index.js": {
                "data": "--- from/src/index.js\n+++ to/src/index.js\n  \
                         export function greet(name) {\n-   return \"Hello, \" \
                         + name;\n+   return \"Hi, \" + name;\n  }",
                "is_diff": true,
            },
            "src/new-name.js": {
                "data": "export const stable = 1;\nexport const alsoStable = 2;\n",
                "is_diff": false,
            },
            "src/removed.js": {
                "data": "--- from/src/removed.js\n+++ /dev/null\n- export const gone = true;\n- ",
                "is_diff": true,
            },
        }),
        "one patch per changed file and none for the one that did not change"
    );
}

#[tokio::test]
async fn a_patch_over_the_cap_is_left_out_and_the_others_are_kept() {
    let store = Memory::new();

    let answer = call(|| store.store().capping_patches_at(100), diffable()).await;
    written_at_least(&store, 2);

    let patches = blob(&store, &patches_of(&answer));

    assert_eq!(
        patches
            .as_object()
            .map(|patches| patches.keys().map(String::as_str).collect::<Vec<&str>>()),
        Some(vec!["src/added.js", "src/new-name.js", "src/removed.js"]),
        "the one patch over the cap is the only one missing, got {patches}"
    );
}

#[tokio::test]
async fn an_entry_that_left_one_patch_out_says_a_patch_was_left_out() {
    let store = Memory::new();

    let answer = call(|| store.store().capping_patches_at(100), diffable()).await;
    written_at_least(&store, 2);

    assert_eq!(
        blob(&store, &meta_of(&answer))["patches_omitted"],
        json!(true),
        "one file's patch was dropped for its size, and the entry has to say \
         so"
    );

    let kept = blob(&store, &patches_of(&answer));
    assert!(
        kept.as_object().is_some_and(|kept| !kept.is_empty()),
        "and the patches that fit are still beside it, which is what makes \
         this a different entry from one that dropped all of them: got {kept}"
    );
}

#[tokio::test]
async fn a_comparison_with_nothing_to_patch_says_nothing_was_dropped() {
    let store = Memory::new();

    let answer = call(|| store.store(), unchanged()).await;
    written_at_least(&store, 2);

    assert_eq!(
        blob(&store, &patches_of(&answer)),
        json!({}),
        "a version against itself changes no file, so nothing is rendered"
    );
    assert_eq!(
        blob(&store, &meta_of(&answer))["patches_omitted"],
        json!(false),
        "and an entry with nothing to patch has dropped nothing"
    );
}

#[tokio::test]
async fn an_entry_over_the_cap_keeps_its_tree_and_says_its_patches_are_gone() {
    let store = Memory::new();

    let answer = call(|| store.store().capping_entries_at(100), diffable()).await;
    written_at_least(&store, 1);

    assert_eq!(
        store.written(),
        vec![meta_of(&answer)],
        "the tree is written and the patches are not"
    );
    assert_eq!(
        blob(&store, &meta_of(&answer))["patches_omitted"],
        json!(true),
        "an entry whose patches were dropped says so"
    );

    let whole = Memory::new();
    let answer = call(|| whole.store(), diffable()).await;
    written_at_least(&whole, 2);

    assert_eq!(
        blob(&whole, &meta_of(&answer))["patches_omitted"],
        json!(false),
        "the same comparison under the real caps keeps them"
    );
}

#[tokio::test]
async fn an_entry_that_dropped_its_patches_is_still_remembered() {
    let store = Memory::new();
    let capped = || store.store().capping_entries_at(100);

    call(capped, diffable()).await;
    written_at_least(&store, 1);

    let second = call(capped, diffable()).await;

    assert_eq!(
        second["structuredContent"]["cached"],
        json!(true),
        "an entry that says its patches were dropped is a hit, got {second}"
    );
}

#[tokio::test]
async fn half_an_entry_is_not_a_cache_hit() {
    let store = Memory::new();

    let answer = call(|| store.store(), diffable()).await;
    written_at_least(&store, 2);

    store.forget(&patches_of(&answer));

    let again = call(|| store.store(), diffable()).await;
    assert_eq!(
        again["structuredContent"]["cached"],
        json!(false),
        "an entry missing a half nothing said was dropped is not a hit, got \
         {again}"
    );

    written_at_least(&store, 2);
    assert_eq!(
        store.written(),
        vec![meta_of(&answer), patches_of(&answer)],
        "and the miss writes back the blob that was gone"
    );
}

#[tokio::test]
async fn an_entry_that_lost_the_patches_it_kept_still_answers_for_every_file() {
    let store = Memory::new();
    let capped = || store.store().capping_patches_at(100);

    let answer = call(capped, diffable()).await;
    written_at_least(&store, 2);

    store.forget(&patches_of(&answer));

    let again = call(capped, diffable()).await;
    assert_eq!(
        again["structuredContent"]["cached"],
        json!(true),
        "an entry that says a patch was dropped is whole without a \
         `patches.json`, and says the same thing when the cap took one: got \
         {again}"
    );

    let asked = json!({
        "handle": answer["structuredContent"]["handle"].clone(),
        "path": "src/added.js",
    });

    let cold = Memory::new();
    assert_eq!(
        call_tool(capped, "get_file_diff", asked.clone()).await,
        call_tool(|| cold.store(), "get_file_diff", asked.clone()).await,
        "and a file whose patch went with the blob is rendered, which is the \
         same answer it has always given"
    );

    let missed = one_sided(capped, "get_file_diff", asked).await;
    assert_eq!(
        missed["isError"],
        json!(true),
        "rendered rather than served, because the patches that did fit are \
         gone with the blob: got {missed}"
    );

    assert_eq!(
        store.written(),
        vec![meta_of(&answer)],
        "and nothing rewrites them, which is what this costs until the entry \
         is evicted"
    );
}

#[tokio::test]
async fn a_store_that_is_not_there_costs_a_recomputed_diff_and_nothing_else() {
    let log = Capture::new();
    let gone = || DiffStore::unavailable().logging_to(log.sink());

    let degraded = call(gone, diffable()).await;
    let again = call(gone, diffable()).await;

    let working = Memory::new();
    let cold = call(|| working.store(), diffable()).await;

    assert_eq!(
        degraded, cold,
        "a diff computed with no store is the diff computed with one"
    );
    assert_eq!(
        again["structuredContent"]["cached"],
        json!(false),
        "and it stays cold, because nothing was ever written: got {again}"
    );

    let notes: Vec<String> = log
        .lines()
        .into_iter()
        .filter(|line| line.contains(r#""seam":"store""#))
        .collect();

    assert!(
        !notes.is_empty(),
        "the store saying it could not answer is what an operator reads: got {:?}",
        log.lines()
    );
    assert!(
        notes
            .iter()
            .all(|note| note.contains("blob store's credentials")),
        "and it says what was not there: got {notes:?}"
    );
}

#[tokio::test]
async fn a_lost_read_is_a_miss_that_says_the_read_failed() {
    let store = Memory::new();

    let answer = call(|| store.store(), diffable()).await;
    written_at_least(&store, 2);

    let log = Capture::new();
    let lost = Memory::failing(&store, Operation::Read);
    let again = call(|| lost.store().logging_to(log.sink()), diffable()).await;
    let notes = store_notes(&log);

    assert_eq!(
        again["structuredContent"]["cached"],
        json!(false),
        "a read the store lost is not a hit, got {again}"
    );
    assert_eq!(
        but_for_cached(again),
        but_for_cached(answer),
        "and the miss is the same comparison, worked out again"
    );
    assert!(
        notes
            .iter()
            .all(|note| note.contains("reading a cached result")),
        "the store says the read is what could not be done: {notes:?}"
    );
}

#[tokio::test]
async fn the_entry_is_written_before_the_answer_is_given() {
    let store = Memory::new().stalling(Duration::from_millis(100));

    let answer = call(|| store.store(), diffable()).await;

    let written = store.written();
    assert!(
        written.contains(&meta_of(&answer)) && written.contains(&patches_of(&answer)),
        "Vercel's Rust runtime gives a function no time after its response, so \
         an entry left for later is an entry never written: got {written:?}"
    );
}

#[tokio::test]
async fn a_write_that_outlasts_its_deadline_is_dropped_and_says_so() {
    let log = Capture::new();
    let slow = Memory::new().stalling(Duration::from_secs(2));
    let hurried = || {
        slow.store()
            .writing_within(Duration::from_millis(50))
            .logging_to(log.sink())
    };

    let began = Instant::now();
    let answer = call(hurried, diffable()).await;
    let answered = began.elapsed();

    assert_eq!(
        answer["isError"],
        json!(false),
        "a write the store could not finish in time is not a failed diff, got {answer}"
    );
    assert!(
        answered < Duration::from_secs(1),
        "the answer waited {answered:?} on a write given 50ms"
    );
    assert!(
        slow.written().is_empty(),
        "a dropped write leaves nothing behind, got {:?}",
        slow.written()
    );
    assert!(
        dropped(&log),
        "a write that was cut short leaves a line saying the entry was dropped, got {:?}",
        log.lines()
    );
}

#[tokio::test]
async fn a_write_never_outlasts_the_call_it_is_part_of() {
    let log = Capture::new();
    let slow = Memory::new().stalling(Duration::from_secs(2));

    let answer = Client::over(
        Ctx::fixture(FIXTURES)
            .storing_in(slow.store().logging_to(log.sink()))
            .answering_within(Duration::from_millis(500)),
    )
    .call(TOOL, diffable())
    .await;

    assert_eq!(
        answer["isError"],
        json!(false),
        "the comparison was computed in time, so a cache write cannot be what \
         turns it into a call out of time, got {answer}"
    );
    assert!(
        dropped(&log),
        "the write gave way to the call's deadline and says so, got {:?}",
        log.lines()
    );
}

fn dropped(log: &Capture) -> bool {
    log.lines()
        .iter()
        .any(|line| line.contains(r#""seam":"store""#) && line.contains("dropped"))
}

#[tokio::test]
async fn an_entry_that_is_already_there_is_not_written_again() {
    let store = Memory::new();

    let answer = call(|| store.store(), diffable()).await;
    written_at_least(&store, 2);

    let uploaded = |at: &str| {
        store
            .uploaded_at(at)
            .unwrap_or_else(|| panic!("`{at}` should be there"))
    };
    let (meta, patches) = (meta_of(&answer), patches_of(&answer));
    let (before_meta, before_patches) = (uploaded(&meta), uploaded(&patches));

    let missed = Memory::failing(&store, Operation::Read);
    let again = call(|| missed.store(), diffable()).await;

    assert_eq!(
        again["structuredContent"]["cached"],
        json!(false),
        "the call has to have missed, or there is no write to skip: got {again}"
    );

    tokio::time::sleep(Duration::from_millis(100)).await;

    assert_eq!(
        (uploaded(&meta), uploaded(&patches)),
        (before_meta, before_patches),
        "the entry was written a second time, and eviction orders on this"
    );
}

#[tokio::test]
async fn changing_anything_the_comparison_is_named_by_is_an_entry_of_its_own() {
    let store = Memory::new();

    let first = call(|| store.store(), diffable()).await;
    written_at_least(&store, 2);

    let named = |answer: &Value| answer["structuredContent"]["diff_id"].clone();

    let variations = [
        ("package", json!({ "package": "churny" })),
        ("from_version", json!({ "from_version": "2.0.0" })),
        ("to_version", json!({ "to_version": "1.0.0" })),
        (
            "similarity_threshold",
            json!({ "similarity_threshold": 0.5 }),
        ),
        ("ignore_whitespace", json!({ "ignore_whitespace": true })),
    ];

    let compared = 1 + variations.len();
    let mut written = 1;

    for (field, change) in variations {
        let mut arguments = diffable();
        for (key, value) in change.as_object().expect("an object of changes") {
            arguments[key] = value.clone();
        }

        let answer = call(|| store.store(), arguments).await;
        written_at_least(&store, 2 * (written + 1));
        written += 1;

        assert_eq!(
            answer["structuredContent"]["cached"],
            json!(false),
            "a different `{field}` is a different comparison, got {answer}"
        );
        assert_ne!(
            named(&answer),
            named(&first),
            "a different `{field}` is named differently, got {answer}"
        );
    }

    assert_eq!(
        store.written().len(),
        2 * compared,
        "one entry per comparison, and an entry is two blobs: {:?}",
        store.written()
    );
}

#[tokio::test]
async fn a_reading_tool_is_served_the_stored_tree_rather_than_the_archives() {
    let store = Memory::new();

    let diffed = call(|| store.store(), diffable()).await;
    written_at_least(&store, 2);
    let walk = json!({ "handle": diffed["structuredContent"]["handle"].clone() });

    let served = one_sided(|| store.store(), "get_diff_tree", walk.clone()).await;
    assert_eq!(
        served["isError"],
        json!(false),
        "the tree was in the store, and the second version is not there to \
         fetch: got {served}"
    );
    assert_eq!(
        served,
        call_tool(|| store.store(), "get_diff_tree", walk.clone()).await,
        "and what it was served is the page the whole fixture set answers \
         with, not a cheaper version of one"
    );

    let empty = Memory::new();
    let missed = one_sided(|| empty.store(), "get_diff_tree", walk).await;
    assert_eq!(
        missed["isError"],
        json!(true),
        "with nothing to be served the same call has to fetch, and fetching \
         is what this fixture set cannot do: got {missed}"
    );
}

#[tokio::test]
async fn a_read_of_the_whole_comparison_is_served_the_stored_tree_too() {
    let store = Memory::new();

    let diffed = call(|| store.store(), diffable()).await;
    written_at_least(&store, 2);
    let uri = format!(
        "diffpack://diff/{}",
        diffed["structuredContent"]["handle"]
            .as_str()
            .unwrap_or_else(|| panic!("the answer carries a handle, got {diffed}"))
    );

    let served = read(ONE_SIDED, || store.store(), &uri).await;
    assert!(
        served.get("error").is_none(),
        "the tree was in the store, and the second version is not there to \
         fetch: got {served}"
    );
    assert_eq!(
        served,
        read(FIXTURES, || store.store(), &uri).await,
        "and it is the document the whole fixture set answers with"
    );

    let empty = Memory::new();
    let missed = read(ONE_SIDED, || empty.store(), &uri).await;
    assert_eq!(
        missed["error"]["code"], NOT_PUBLISHED,
        "with nothing to be served the same read has to fetch, and the version \
         it goes for is not published in this set: got {missed}"
    );
}

#[tokio::test]
async fn a_file_diff_is_served_the_stored_patch_rather_than_the_archives() {
    let store = Memory::new();

    let diffed = call(|| store.store(), diffable()).await;
    written_at_least(&store, 2);
    let asked = json!({
        "handle": diffed["structuredContent"]["handle"].clone(),
        "path": "src/index.js",
    });

    let served = one_sided(|| store.store(), "get_file_diff", asked.clone()).await;
    assert_eq!(
        served["isError"],
        json!(false),
        "the patch was in the store, and the second version is not there to \
         fetch: got {served}"
    );

    let cold = Memory::new();
    assert_eq!(
        served,
        call_tool(|| cold.store(), "get_file_diff", asked.clone()).await,
        "and it is the patch the tool renders from both archives, which is \
         the whole of what a remembered one is allowed to be"
    );

    let empty = Memory::new();
    let missed = one_sided(|| empty.store(), "get_file_diff", asked).await;
    assert_eq!(
        missed["isError"],
        json!(true),
        "with nothing to be served the same call has to fetch, and fetching \
         is what this fixture set cannot do: got {missed}"
    );
}

#[tokio::test]
async fn a_remembered_patch_says_whether_its_files_decoded() {
    let store = Memory::new();

    let diffed = call(
        || store.store(),
        json!({
            "registry": "npm",
            "package": "undecodable",
            "from_version": "1.0.0",
            "to_version": "2.0.0",
        }),
    )
    .await;
    written_at_least(&store, 2);
    let patches = blob(&store, &patches_of(&diffed));

    for path in ["logo.png", "src/far.js"] {
        assert!(
            patches[path].is_object(),
            "`{path}` changed, so the entry holds its patch and the warm call \
             below is served it: got {patches}"
        );

        let asked = json!({
            "handle": diffed["structuredContent"]["handle"].clone(),
            "path": path,
        });
        let warm = call_tool(|| store.store(), "get_file_diff", asked.clone()).await;
        let cold = Memory::new();

        assert_eq!(
            warm["structuredContent"]["validUtf8"],
            json!(false),
            "`{path}`: got {warm}"
        );
        assert_eq!(
            warm,
            call_tool(|| cold.store(), "get_file_diff", asked).await,
            "`{path}`: a remembered patch is the rendered one, flag and all"
        );
    }
}

#[tokio::test]
async fn a_renamed_file_is_served_only_the_patch_it_was_asked_for() {
    let store = Memory::new();

    let diffed = call(|| store.store(), diffable()).await;
    written_at_least(&store, 2);
    let handle = || diffed["structuredContent"]["handle"].clone();

    let moved = json!({
        "handle": handle(),
        "path": "src/new-name.js",
        "old_path": "src/old-name.js",
    });
    let served = one_sided(|| store.store(), "get_file_diff", moved).await;
    assert_eq!(
        served["isError"],
        json!(false),
        "the patch for the file as the tree names it is the one the entry \
         holds: got {served}"
    );

    let bare = json!({ "handle": handle(), "path": "src/new-name.js" });
    let cold = Memory::new();
    assert_eq!(
        call_tool(|| store.store(), "get_file_diff", bare.clone()).await,
        call_tool(|| cold.store(), "get_file_diff", bare).await,
        "a file asked about without the `old_path` it moved from is the same \
         answer warm and cold, and it is not the entry's patch"
    );
}

#[tokio::test]
async fn a_directory_is_refused_out_of_the_entry_without_the_archives() {
    let store = Memory::new();

    let diffed = call(|| store.store(), diffable()).await;
    written_at_least(&store, 2);
    let asked = json!({
        "handle": diffed["structuredContent"]["handle"].clone(),
        "path": "src",
    });

    let served = one_sided(|| store.store(), "get_file_diff", asked.clone()).await;
    let cold = Memory::new();
    assert_eq!(
        served,
        call_tool(|| cold.store(), "get_file_diff", asked.clone()).await,
        "the tree says `src` is a directory, and the second version is not \
         there to fetch: got {served}"
    );
    assert_eq!(
        served["isError"],
        json!(true),
        "and that answer is the refusal a directory gets: got {served}"
    );

    let empty = Memory::new();
    let missed = one_sided(|| empty.store(), "get_file_diff", asked).await;
    assert_eq!(
        missed["isError"],
        json!(true),
        "with nothing to be served the same call has to fetch, and fetching \
         is what this fixture set cannot do: got {missed}"
    );
    assert_ne!(
        missed, served,
        "and that failure is the fetch, not the directory refusal"
    );
}

#[tokio::test]
async fn a_directory_is_refused_out_of_the_entry_whatever_old_path_names() {
    let store = Memory::new();

    let diffed = call(|| store.store(), diffable()).await;
    written_at_least(&store, 2);

    for old_path in ["src/new-name.js", "src/old-name.js"] {
        let asked = json!({
            "handle": diffed["structuredContent"]["handle"].clone(),
            "path": "src",
            "old_path": old_path,
        });

        let served = one_sided(|| store.store(), "get_file_diff", asked.clone()).await;
        let cold = Memory::new();
        assert_eq!(
            served,
            call_tool(|| cold.store(), "get_file_diff", asked).await,
            "`src` from `{old_path}`: the tree says `src` is a directory with \
             no file at it, and the second version is not there to fetch: got \
             {served}"
        );
        assert_eq!(
            served["isError"],
            json!(true),
            "`src` from `{old_path}`: and that answer is the refusal a \
             directory gets: got {served}"
        );
    }
}

#[tokio::test]
async fn a_read_of_one_files_diff_is_served_the_stored_patch_too() {
    let store = Memory::new();

    let diffed = call(|| store.store(), diffable()).await;
    written_at_least(&store, 2);
    let uri = format!(
        "diffpack://diff/{}/file/src/index.js",
        diffed["structuredContent"]["handle"]
            .as_str()
            .unwrap_or_else(|| panic!("the answer carries a handle, got {diffed}"))
    );

    let served = read(ONE_SIDED, || store.store(), &uri).await;
    assert!(
        served.get("error").is_none(),
        "the patch was in the store, and the second version is not there to \
         fetch: got {served}"
    );

    let cold = Memory::new();
    assert_eq!(
        served,
        read(FIXTURES, || cold.store(), &uri).await,
        "and it is the document the tool's own rendering answers with"
    );

    let empty = Memory::new();
    let missed = read(ONE_SIDED, || empty.store(), &uri).await;
    assert_eq!(
        missed["error"]["code"], NOT_PUBLISHED,
        "with nothing to be served the same read has to fetch, and the version \
         it goes for is not published in this set: got {missed}"
    );
}

#[tokio::test]
async fn a_read_of_a_directory_is_refused_out_of_the_entry_too() {
    let store = Memory::new();

    let diffed = call(|| store.store(), diffable()).await;
    written_at_least(&store, 2);
    let uri = format!(
        "diffpack://diff/{}/file/src",
        diffed["structuredContent"]["handle"]
            .as_str()
            .unwrap_or_else(|| panic!("the answer carries a handle, got {diffed}"))
    );

    let served = read(ONE_SIDED, || store.store(), &uri).await;
    let cold = Memory::new();
    assert_eq!(
        served,
        read(FIXTURES, || cold.store(), &uri).await,
        "the tree says `src` is a directory, and the second version is not \
         there to fetch: got {served}"
    );
    assert!(
        served.get("error").is_some(),
        "and that answer is the refusal a directory gets: got {served}"
    );

    let empty = Memory::new();
    let missed = read(ONE_SIDED, || empty.store(), &uri).await;
    assert_eq!(
        missed["error"]["code"], NOT_PUBLISHED,
        "with nothing to be served the same read has to fetch, and the version \
         it goes for is not published in this set: got {missed}"
    );
    assert_ne!(
        missed, served,
        "and that failure is the fetch, not the directory refusal"
    );
}

#[tokio::test]
async fn a_file_that_became_a_directory_is_served_cold() {
    let served = lib_cold("1.0.0", "2.0.0").await;

    is_the_patch_of_lib(&served, "--- from/lib\n+++ /dev/null\n");
}

#[tokio::test]
async fn a_file_that_became_a_directory_is_served_warm_without_the_archives() {
    let served = lib_warm("1.0.0", "2.0.0").await;

    assert_eq!(
        served,
        lib_cold("1.0.0", "2.0.0").await,
        "a stored comparison answers `lib` as a fresh one does, and without \
         fetching 2.0.0, which this fixture set does not have: got {served}"
    );
    is_the_patch_of_lib(&served, "--- from/lib\n+++ /dev/null\n");
}

#[tokio::test]
async fn a_file_where_a_directory_was_is_served_cold() {
    let served = lib_cold("2.0.0", "1.0.0").await;

    is_the_patch_of_lib(&served, "--- /dev/null\n+++ to/lib\n");
}

#[tokio::test]
async fn a_file_where_a_directory_was_is_served_warm_without_the_archives() {
    let served = lib_warm("2.0.0", "1.0.0").await;

    assert_eq!(
        served,
        lib_cold("2.0.0", "1.0.0").await,
        "a stored comparison answers `lib` as a fresh one does, and without \
         fetching 2.0.0, which this fixture set does not have: got {served}"
    );
    is_the_patch_of_lib(&served, "--- /dev/null\n+++ to/lib\n");
}

#[tokio::test]
async fn a_read_of_a_path_that_is_a_file_and_a_directory_is_the_tools_answer() {
    let directions = [
        ("1.0.0", "2.0.0", "--- from/lib\n+++ /dev/null\n"),
        ("2.0.0", "1.0.0", "--- /dev/null\n+++ to/lib\n"),
    ];

    for (from, to, header) in directions {
        let store = Memory::new();
        let diffed = call(|| store.store(), shape(from, to)).await;
        written_at_least(&store, 2);
        let handle = diffed["structuredContent"]["handle"]
            .as_str()
            .unwrap_or_else(|| panic!("the answer carries a handle, got {diffed}"))
            .to_owned();
        let asked = json!({ "handle": handle, "path": "lib" });
        let uri = format!("diffpack://diff/{handle}/file/lib");

        let cold = || Memory::new().store();
        let cells = [
            (
                "warm",
                one_sided(|| store.store(), "get_file_diff", asked.clone()).await,
                read(ONE_SIDED, || store.store(), &uri).await,
            ),
            (
                "cold",
                call_tool(cold, "get_file_diff", asked.clone()).await,
                read(FIXTURES, cold, &uri).await,
            ),
        ];

        for (when, called, read) in cells {
            is_the_patch_of_lib(&called, header);
            assert_eq!(
                read["result"]["contents"][0]["text"], called["structuredContent"]["text"],
                "{from} → {to} {when}: the resource answers `lib` with the \
                 tool's patch, got {read} against {called}"
            );
        }
    }
}

#[tokio::test]
async fn a_file_inside_a_path_that_became_a_directory_is_served_warm_without_the_archives() {
    let store = Memory::new();
    let diffed = call(|| store.store(), shape("1.0.0", "2.0.0")).await;
    written_at_least(&store, 2);
    let asked = json!({
        "handle": diffed["structuredContent"]["handle"].clone(),
        "path": "lib/index.js",
    });

    let served = one_sided(|| store.store(), "get_file_diff", asked.clone()).await;

    assert_eq!(
        served["isError"],
        json!(false),
        "the entry holds `lib/index.js`'s patch: got {served}"
    );
    let cold = Memory::new();
    assert_eq!(
        served,
        call_tool(|| cold.store(), "get_file_diff", asked).await,
        "and it is the patch a fresh comparison renders"
    );
}

#[tokio::test]
async fn an_entry_holds_the_patch_for_a_file_that_became_a_directory() {
    let store = Memory::new();

    let answer = call(|| store.store(), shape("1.0.0", "2.0.0")).await;
    written_at_least(&store, 2);

    let patches = blob(&store, &patches_of(&answer));
    assert_eq!(
        patches["lib"],
        json!({
            "data": "--- from/lib\n+++ /dev/null\n- A plain file, where 2.0.0 has a directory.\n- ",
            "is_diff": true,
        }),
        "`lib` is a file in 1.0.0 and removed in 2.0.0, so the entry holds its \
         removal: got {patches}"
    );
    assert!(
        patches.get("lib/index.js").is_some(),
        "the file added inside the directory has its patch: got {patches}"
    );
}

fn shape(from: &str, to: &str) -> Value {
    json!({
        "registry": "npm",
        "package": "shape",
        "from_version": from,
        "to_version": to,
    })
}

async fn shape_handle(from: &str, to: &str) -> Value {
    let minted = Memory::new();
    let diffed = call(|| minted.store(), shape(from, to)).await;

    diffed["structuredContent"]["handle"].clone()
}

async fn lib_cold(from: &str, to: &str) -> Value {
    let asked = json!({ "handle": shape_handle(from, to).await, "path": "lib" });

    let cold = Memory::new();
    call_tool(|| cold.store(), "get_file_diff", asked).await
}

async fn lib_warm(from: &str, to: &str) -> Value {
    let store = Memory::new();
    let diffed = call(|| store.store(), shape(from, to)).await;
    written_at_least(&store, 2);
    let asked = json!({
        "handle": diffed["structuredContent"]["handle"].clone(),
        "path": "lib",
    });

    one_sided(|| store.store(), "get_file_diff", asked).await
}

fn is_the_patch_of_lib(answer: &Value, header: &str) {
    assert_eq!(
        answer["isError"],
        json!(false),
        "`lib` is a file in 1.0.0, so it has a patch: got {answer}"
    );

    let patch = &answer["structuredContent"];
    let text = patch["text"]
        .as_str()
        .unwrap_or_else(|| panic!("a patch carries its text, got {answer}"));
    assert!(
        text.starts_with(header) && text.contains("A plain file, where 2.0.0 has a directory."),
        "the answer is the file `lib`, under the header `{header}`: got {text}"
    );
    assert_eq!(
        patch["isDiff"],
        json!(true),
        "and it is a diff: got {answer}"
    );
}

#[tokio::test]
async fn a_file_whose_patch_was_dropped_is_rendered_rather_than_missed() {
    let store = Memory::new();
    let capped = || store.store().capping_patches_at(100);

    let diffed = call(capped, diffable()).await;
    written_at_least(&store, 2);
    let asking = |path: &str| {
        json!({
            "handle": diffed["structuredContent"]["handle"].clone(),
            "path": path,
        })
    };

    let kept = one_sided(capped, "get_file_diff", asking("src/added.js")).await;
    assert_eq!(
        kept["isError"],
        json!(false),
        "a patch under the cap is in the entry and is served: got {kept}"
    );

    let dropped = one_sided(capped, "get_file_diff", asking("src/index.js")).await;
    assert_eq!(
        dropped["isError"],
        json!(true),
        "the file the cap took has to be fetched, and fetching is what this \
         fixture set cannot do: got {dropped}"
    );

    let cold = Memory::new();
    assert_eq!(
        call_tool(capped, "get_file_diff", asking("src/index.js")).await,
        call_tool(|| cold.store(), "get_file_diff", asking("src/index.js")).await,
        "and where it can, the answer is the patch it always was"
    );
}

#[tokio::test]
async fn the_tool_and_the_resource_answer_every_kind_of_file_alike() {
    let store = Memory::new();
    let capped = || store.store().capping_patches_at(100);

    let diffed = call(capped, diffable()).await;
    written_at_least(&store, 2);
    let handle = diffed["structuredContent"]["handle"]
        .as_str()
        .unwrap_or_else(|| panic!("the answer carries a handle, got {diffed}"))
        .to_owned();

    let cold = || Memory::new().store();
    let warm_and_cold: [(&str, &dyn Fn() -> DiffStore); 2] = [("warm", &capped), ("cold", &cold)];

    for (when, store) in warm_and_cold {
        for (path, old_path) in [
            ("src/added.js", None),
            ("src/index.js", None),
            ("README.md", None),
            ("src/new-name.js", Some("src/old-name.js")),
            ("nowhere/at/all.js", None),
        ] {
            let mut asked = json!({ "handle": handle, "path": path });
            if let Some(old_path) = old_path {
                asked["old_path"] = json!(old_path);
            }
            let called = call_tool(store, "get_file_diff", asked).await;
            let called = &called["structuredContent"];

            let uri = format!("diffpack://diff/{handle}/file/{path}");
            let read = read(FIXTURES, store, &uri).await;
            let document = &read["result"]["contents"][0];

            assert!(
                called["text"].is_string(),
                "`{path}` {when}: every one of these is an answer, not a \
                 failure the two could agree on, got {called}"
            );
            assert_eq!(
                document["text"], called["text"],
                "`{path}` {when}: the resource and the tool should give one \
                 answer, got {read} against {called}"
            );
            assert_eq!(
                document["mimeType"],
                json!(if called["isDiff"] == json!(true) {
                    "text/x-diff"
                } else {
                    "text/plain"
                }),
                "`{path}` {when}: the media type says what `isDiff` says, got \
                 {read} against {called}"
            );
        }
    }

    let bare = json!({ "handle": handle, "path": "src/new-name.js" });
    let warm = call_tool(capped, "get_file_diff", bare.clone()).await;
    assert_eq!(
        warm,
        call_tool(cold, "get_file_diff", bare).await,
        "a renamed file asked about without its `old_path` is the same answer \
         warm and cold"
    );
    let read = read(
        FIXTURES,
        capped,
        &format!("diffpack://diff/{handle}/file/src/new-name.js"),
    )
    .await;
    assert_ne!(
        read["result"]["contents"][0]["text"], warm["structuredContent"]["text"],
        "and it is not the rename the resource reads out of the tree"
    );
}

#[tokio::test]
async fn a_reading_tool_answers_the_same_with_no_store_to_read() {
    let log = Capture::new();
    let gone = || DiffStore::unavailable().logging_to(log.sink());

    let store = Memory::new();
    let diffed = call(|| store.store(), diffable()).await;
    let walk = json!({ "handle": diffed["structuredContent"]["handle"].clone() });

    let degraded = call_tool(gone, "get_diff_tree", walk.clone()).await;
    let cached = call_tool(|| store.store(), "get_diff_tree", walk).await;

    assert_eq!(
        degraded["isError"],
        json!(false),
        "a walk with no store to read is a walk, got {degraded}"
    );
    assert_eq!(
        degraded, cached,
        "and it is the page a store would have been served"
    );

    let notes = store_notes(&log);
    assert!(
        notes
            .iter()
            .all(|note| note.contains("blob store's credentials")),
        "what an operator reads is the store saying what was not there: got \
         {notes:?}"
    );
}

#[tokio::test]
async fn a_budget_with_room_for_one_entry_keeps_the_newer_one() {
    let store = Memory::new();

    let first = call(|| store.store(), at(0)).await;
    landed(&store, &first);

    let entry = held(&store);
    let budgeted = || store.store().budgeting(entry + entry / 16, entry);

    let second = call(budgeted, at(1)).await;
    landed(&store, &second);

    assert_eq!(
        store.written(),
        vec![meta_of(&second), patches_of(&second)],
        "the older comparison should have made way for the newer one"
    );
}

#[tokio::test]
async fn a_run_well_past_the_budget_never_takes_the_store_over_it() {
    let store = Memory::new();

    let first = call(|| store.store(), at(0)).await;
    landed(&store, &first);

    let entry = held(&store);
    let (max, target) = (3 * entry + entry / 16, 3 * entry);
    let budgeted = || store.store().budgeting(max, target);

    for step in 1..12 {
        let answer = call(budgeted, at(step)).await;
        landed(&store, &answer);

        assert!(
            held(&store) <= max,
            "after {step} more comparisons the store holds {} bytes against a \
             budget of {max}",
            held(&store)
        );
    }

    assert!(
        held(&store) >= entry,
        "a store that evicted everything would pass the check above without \
         being a cache: it holds {} bytes",
        held(&store)
    );
}

#[tokio::test]
async fn a_sweep_takes_the_oldest_entries_whole_and_the_newest_survive() {
    let store = Memory::new();

    let first = call(|| store.store(), at(0)).await;
    landed(&store, &first);
    let entry = held(&store);

    let mut seeded = vec![first];
    for step in 1..5 {
        let answer = call(|| store.store(), at(step)).await;
        landed(&store, &answer);
        seeded.push(answer);
    }

    assert_eq!(
        held(&store),
        5 * entry,
        "the five weigh the same, which is what makes what a sweep deletes \
         predictable rather than a race between sizes"
    );

    let budgeted = || store.store().budgeting(2 * entry + entry / 16, 2 * entry);
    let sixth = call(budgeted, at(5)).await;
    landed(&store, &sixth);

    let mut survived: Vec<String> = [&seeded[4], &sixth]
        .into_iter()
        .flat_map(|answer| [meta_of(answer), patches_of(answer)])
        .collect();
    survived.sort();

    assert_eq!(
        store.written(),
        survived,
        "the newest two entries, both blobs of each, and nothing else"
    );
}

#[tokio::test]
async fn a_put_of_an_entry_already_there_makes_no_room_it_will_not_use() {
    let store = Memory::new();

    let first = call(|| store.store(), at(0)).await;
    landed(&store, &first);
    let entry = held(&store);

    let second = call(|| store.store(), at(1)).await;
    landed(&store, &second);

    let missed = Memory::failing(&store, Operation::Read);
    let budgeted = || missed.store().budgeting(2 * entry + entry / 16, 2 * entry);

    let again = call(budgeted, at(1)).await;
    assert_eq!(
        again["structuredContent"]["cached"],
        json!(false),
        "the call has to have missed, or there is no second put to make: got {again}"
    );

    tokio::time::sleep(Duration::from_millis(100)).await;

    let mut expected: Vec<String> = [&first, &second]
        .into_iter()
        .flat_map(|answer| [meta_of(answer), patches_of(answer)])
        .collect();
    expected.sort();

    assert_eq!(
        store.written(),
        expected,
        "putting an entry that is already there asked for room the write then \
         skipped, and the older comparison paid for it"
    );
}

#[tokio::test]
async fn a_put_of_the_oldest_entry_does_not_sweep_itself_away() {
    let store = Memory::new();

    let oldest = call(|| store.store(), at(0)).await;
    landed(&store, &oldest);
    let entry = held(&store);

    let newer = call(|| store.store(), at(1)).await;
    landed(&store, &newer);

    let uploaded = |at: &str| {
        store
            .uploaded_at(at)
            .unwrap_or_else(|| panic!("`{at}` should be there, {:?} is", store.written()))
    };
    let (meta, patches) = (meta_of(&oldest), patches_of(&oldest));
    let before = (uploaded(&meta), uploaded(&patches));

    let missed = Memory::failing(&store, Operation::Read);
    let budgeted = || missed.store().budgeting(2 * entry + entry / 16, 2 * entry);

    let again = call(budgeted, at(0)).await;
    assert_eq!(
        again["structuredContent"]["cached"],
        json!(false),
        "the call has to have missed, or there is no second put to make: got {again}"
    );

    tokio::time::sleep(Duration::from_millis(100)).await;

    assert_eq!(
        (uploaded(&meta), uploaded(&patches)),
        before,
        "the sweep deleted the entry being put and the write put it back, which \
         against a store whose deletes propagate later is the entry lost"
    );
}

#[tokio::test]
async fn an_entry_too_big_for_the_whole_budget_is_refused_without_a_sweep() {
    let store = Memory::new();

    let first = call(|| store.store(), at(0)).await;
    landed(&store, &first);
    let entry = held(&store);

    let log = Capture::new();
    let budgeted = || {
        store
            .store()
            .budgeting(entry / 2, entry / 4)
            .logging_to(log.sink())
    };

    let answered = call(budgeted, at(1)).await;
    let notes = store_notes(&log);

    assert_eq!(
        store.written(),
        vec![meta_of(&first), patches_of(&first)],
        "the entry already there is untouched: a sweep that emptied the cache \
         and still had no room would have spent it for nothing"
    );
    assert_eq!(
        answered["isError"],
        json!(false),
        "and an entry the cache will not hold is still a comparison that was \
         answered, got {answered}"
    );
    assert!(
        notes
            .iter()
            .all(|note| note.contains("larger than the whole budget")),
        "the refusal says what it would not take, and says: {notes:?}"
    );
}

#[tokio::test]
async fn a_sweep_whose_deletes_all_fail_admits_nothing() {
    let store = Memory::new();

    let first = call(|| store.store(), at(0)).await;
    landed(&store, &first);
    let entry = held(&store);

    let log = Capture::new();
    let stubborn = Memory::failing(&store, Operation::Delete);
    let budgeted = || {
        stubborn
            .store()
            .budgeting(entry + entry / 16, entry)
            .logging_to(log.sink())
    };

    let second = call(budgeted, at(1)).await;
    let notes = store_notes(&log);
    kept_out(&store, &second);

    assert_eq!(
        store.written(),
        vec![meta_of(&first), patches_of(&first)],
        "a sweep that freed nothing made no room to admit anything into"
    );
    assert!(
        notes.iter().all(|note| note.contains("evicting an entry")),
        "a sweep that could not delete says so, and says: {notes:?}"
    );
}

#[tokio::test]
async fn a_sweep_that_freed_less_than_it_needed_admits_nothing() {
    let store = Memory::new();

    let first = call(|| store.store(), at(0)).await;
    landed(&store, &first);
    let entry = held(&store);

    let mut seeded = vec![first];
    for step in 1..5 {
        let answer = call(|| store.store(), at(step)).await;
        landed(&store, &answer);
        seeded.push(answer);
    }

    assert_eq!(
        held(&store),
        5 * entry,
        "the five weigh the same, which is what makes what a sweep frees a          number this test can state"
    );

    let log = Capture::new();
    let stubborn = Memory::failing_after(&store, Operation::Delete, 1);
    let budgeted = || {
        stubborn
            .store()
            .budgeting(3 * entry, 2 * entry)
            .logging_to(log.sink())
    };

    let sixth = call(budgeted, at(5)).await;
    let notes = store_notes(&log);
    kept_out(&store, &sixth);

    let mut survived: Vec<String> = seeded[1..]
        .iter()
        .flat_map(|answer| [meta_of(answer), patches_of(answer)])
        .collect();
    survived.sort();

    assert_eq!(
        store.written(),
        survived,
        "the one delete that worked took the oldest entry, and the entry it          was making room for was refused"
    );
    assert!(
        notes.iter().all(|note| note.contains("evicting an entry")),
        "every delete after the first says it could not happen: {notes:?}"
    );
}

#[tokio::test]
async fn a_listing_that_fails_refuses_admission_and_writes_nothing() {
    let store = Memory::new();

    let log = Capture::new();
    let blind = Memory::failing(&store, Operation::List);
    let answer = call(|| blind.store().logging_to(log.sink()), diffable()).await;
    let notes = store_notes(&log);
    kept_out(&store, &answer);

    assert_eq!(
        answer["isError"],
        json!(false),
        "a store that cannot list is still a comparison that was answered, got \
         {answer}"
    );
    assert_eq!(
        store.written(),
        Vec::<String>::new(),
        "an entry admitted against a total nobody knows is how the ceiling \
         gets exceeded"
    );
    assert!(
        notes.iter().all(|note| note.contains("listing what")),
        "the refusal says the listing is what could not be done: {notes:?}"
    );
}

#[tokio::test]
async fn a_head_that_fails_for_one_blob_of_an_entry_writes_neither_blob() {
    let store = Memory::new();

    let log = Capture::new();
    let unsure = Memory::failing_once(&store, Operation::Head);
    let answer = call(|| unsure.store().logging_to(log.sink()), diffable()).await;
    let notes = store_notes(&log);
    kept_out(&store, &answer);

    assert_eq!(
        answer["isError"],
        json!(false),
        "a store that cannot say what it holds is still a comparison that was \
         answered, got {answer}"
    );
    assert_eq!(
        store.written(),
        Vec::<String>::new(),
        "the blob the store did answer for was written without its partner"
    );
    assert!(
        notes.iter().all(|note| note.contains("writing to")),
        "the store says the write is what could not be done: {notes:?}"
    );
}

#[tokio::test]
async fn a_put_that_fails_still_answers_the_diff_and_says_so() {
    let store = Memory::new();

    let log = Capture::new();
    let refusing = Memory::failing(&store, Operation::Write);
    let answer = call(|| refusing.store().logging_to(log.sink()), diffable()).await;
    let notes = store_notes(&log);

    let working = Memory::new();
    let cold = call(|| working.store(), diffable()).await;

    assert_eq!(
        answer, cold,
        "a diff whose entry could not be written is the diff computed with a \
         store that could"
    );
    assert_eq!(
        store.written(),
        Vec::<String>::new(),
        "a put the store refused left something behind"
    );
    assert!(
        notes.iter().all(|note| note.contains("writing to")),
        "the store says the write is what could not be done: {notes:?}"
    );
}

fn kept_out(store: &Memory, answer: &Value) {
    let meta = meta_of(answer);

    assert!(
        !store.written().contains(&meta),
        "the entry at `{meta}` was admitted into room the sweep never freed"
    );
}

fn store_notes(log: &Capture) -> Vec<String> {
    let notes: Vec<String> = log
        .lines()
        .into_iter()
        .filter(|line| line.contains(r#""seam":"store""#))
        .collect();

    assert!(
        !notes.is_empty(),
        "the store has said something by the time the answer is in, {:?} is all there is",
        log.lines()
    );
    notes
}

#[tokio::test]
async fn two_admissions_at_once_do_not_take_the_store_over_the_ceiling() {
    let store = Memory::new();

    let first = call(|| store.store(), at(0)).await;
    landed(&store, &first);
    let entry = held(&store);

    for step in 1..4 {
        let answer = call(|| store.store(), at(step)).await;
        landed(&store, &answer);
    }

    let (max, target) = (4 * entry, 3 * entry);
    let slow = store.clone().stalling(Duration::from_millis(200));
    let budgeted = || slow.store().budgeting(max, target);

    let (fifth, sixth) = tokio::join!(call(budgeted, at(4)), call(budgeted, at(5)));
    landed(&store, &fifth);
    landed(&store, &sixth);

    assert!(
        held(&store) <= max,
        "two entries admitted at once left the store holding {} bytes against \
         a ceiling of {max}",
        held(&store)
    );
}

#[tokio::test]
async fn the_sweep_is_done_before_the_answer_is_given() {
    let store = Memory::new();

    let oldest = call(|| store.store(), at(0)).await;
    landed(&store, &oldest);
    let entry = held(&store);

    let second = call(|| store.store(), at(1)).await;
    landed(&store, &second);

    let slow = store.clone().stalling(Duration::from_millis(100));
    let budgeted = || slow.store().budgeting(entry + entry / 16, entry);
    let third = call(budgeted, at(2)).await;

    assert_eq!(
        third["isError"],
        json!(false),
        "the comparison is the answer, got {third}"
    );
    let written = store.written();
    assert!(
        !written.contains(&meta_of(&oldest)),
        "the room the third entry needed is made before the answer, or it is \
         never made: {written:?}"
    );
    assert!(
        written.contains(&meta_of(&third)) && written.contains(&patches_of(&third)),
        "and the third entry is in it: {written:?}"
    );
}

#[tokio::test]
async fn a_comparison_the_sweep_took_is_recomputed_and_served() {
    let store = Memory::new();

    let diffed = call(|| store.store(), at(0)).await;
    landed(&store, &diffed);
    let entry = held(&store);

    let handle = diffed["structuredContent"]["handle"].clone();
    let walk = json!({ "handle": handle });
    let warm = call_tool(|| store.store(), "get_diff_tree", walk.clone()).await;

    let budgeted = || store.store().budgeting(entry + entry / 16, entry);
    let next = call(budgeted, at(1)).await;
    landed(&store, &next);

    assert!(
        !store.written().contains(&meta_of(&diffed)),
        "the sweep should have taken the first comparison: {:?}",
        store.written()
    );

    let evicted = call_tool(budgeted, "get_diff_tree", walk).await;

    assert_eq!(
        evicted["isError"],
        json!(false),
        "a handle whose entry was swept is a comparison this server can make \
         again, got {evicted}"
    );
    assert_eq!(
        evicted, warm,
        "and it is the same walk it was before the sweep, so eviction costs \
         a recomputation and nothing an agent can see"
    );
}

fn at(step: u32) -> Value {
    let hundredths = 51 + step + step / 9;

    let mut arguments = diffable();
    arguments["similarity_threshold"] = json!(f64::from(hundredths) / 100.0);
    arguments
}

fn held(store: &Memory) -> u64 {
    store
        .written()
        .iter()
        .filter_map(|pathname| store.blob(pathname))
        .map(|bytes| bytes.len() as u64)
        .sum()
}

fn landed(store: &Memory, answer: &Value) {
    let (meta, patches) = (meta_of(answer), patches_of(answer));
    let written = store.written();

    assert!(
        written.contains(&meta) && written.contains(&patches),
        "the entry at `{meta}` is written before the answer, and {written:?} is what is there"
    );
}

fn written_at_least(store: &Memory, blobs: usize) {
    assert!(
        store.written().len() >= blobs,
        "the store holds {blobs} blobs once the answer is in, it holds {:?}",
        store.written()
    );
}

fn patches_of(answer: &Value) -> String {
    format!("{}/patches.json", under(answer))
}

fn meta_of(answer: &Value) -> String {
    format!("{}/meta.json", under(answer))
}

fn under(answer: &Value) -> String {
    let diff_id = answer["structuredContent"]["diff_id"]
        .as_str()
        .unwrap_or_else(|| panic!("the answer names the comparison, got {answer}"));

    format!("diffs/v1/{diff_id}")
}

fn blob(store: &Memory, pathname: &str) -> Value {
    let bytes = store
        .blob(pathname)
        .unwrap_or_else(|| panic!("`{pathname}` should be there, {:?} is", store.written()));

    serde_json::from_slice(&bytes)
        .unwrap_or_else(|e| panic!("`{pathname}` should be JSON, got {e}"))
}

fn but_for_cached(mut answer: Value) -> Value {
    let mirrored = answer["content"][0]["text"]
        .as_str()
        .unwrap_or_else(|| panic!("a text block, got {answer}"))
        .to_owned();
    answer["content"][0]["text"] =
        serde_json::from_str(&mirrored).unwrap_or_else(|e| panic!("the mirror is JSON: {e}"));

    for at in ["/structuredContent", "/content/0/text"] {
        let copy = answer
            .pointer_mut(at)
            .and_then(Value::as_object_mut)
            .unwrap_or_else(|| panic!("an object at `{at}` to take `cached` out of"));

        assert!(
            copy.remove("cached")
                .is_some_and(|cached| cached.is_boolean()),
            "every copy of an answer says whether it was cached, `{at}` does not"
        );
    }

    answer
}

async fn call(store: impl Fn() -> DiffStore, arguments: Value) -> Value {
    call_tool(store, TOOL, arguments).await
}

async fn call_tool(store: impl Fn() -> DiffStore, tool: &str, arguments: Value) -> Value {
    calling(FIXTURES, store, tool, arguments).await
}

async fn one_sided(store: impl Fn() -> DiffStore, tool: &str, arguments: Value) -> Value {
    calling(ONE_SIDED, store, tool, arguments).await
}

async fn calling(
    fixtures: &str,
    store: impl Fn() -> DiffStore,
    tool: &str,
    arguments: Value,
) -> Value {
    Client::over(Ctx::fixture(fixtures).storing_in(store()))
        .call(tool, arguments)
        .await
}

async fn read(fixtures: &str, store: impl Fn() -> DiffStore, uri: &str) -> Value {
    Client::over(Ctx::fixture(fixtures).storing_in(store()))
        .post(json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "resources/read",
            "params": { "uri": uri },
        }))
        .await
}
