//! Cached diff results, driven the way an agent drives them.
//!
//! The cache has no surface of its own: nothing calls it, nothing reads it
//! back, and the only thing an agent ever sees of it is that the same
//! comparison asked for twice was cheap the second time. So every test here
//! goes over the wire — `tools/call` on `diff_package_versions`, through the
//! service factory `router_with` takes — and the store under it is an
//! in-process one the test holds a handle to, the way `tests/log.rs` holds a
//! [`Capture`](diffpack_server::log::Capture).
//!
//! That handle is what lets a test say more than "it was cheap": it names the
//! blobs an entry is, and when each of them was written. Both are the
//! cache's contract rather than its internals — the pathnames are what #27
//! reads a result back from, and the upload moment is the order #22 evicts
//! in.
//!
//! # What a test here is not
//!
//! A test of the Vercel Blob client. That is #20's, in `src/store/blob.rs`,
//! against a stub HTTP server, because the client is private to that module.
//! What is asserted here is the policy above it — which blobs an entry is,
//! when they are written, what happens when they are too big, and what
//! happens when the store is not there — and that policy is the same code
//! whichever adapter is underneath.

use std::time::{Duration, Instant};

use axum::body::Body;
use axum::http::Request;
use diffpack_server::log::Capture;
use diffpack_server::mcp::Diffpack;
use diffpack_server::router;
use diffpack_server::store::{DiffStore, Memory};
use diffpack_server::tools::Ctx;
use http_body_util::BodyExt;
use serde_json::{json, Value};
use tower::ServiceExt;

const CURRENT: &str = "2026-07-28";

const TOOL: &str = "diff_package_versions";

/// The fixture sets this suite is served from, instead of the registries.
const FIXTURES: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/fixtures");

/// The pair every test here compares unless it needs another.
///
/// `diffable` 1.0.0 → 2.0.0 is the same pair `tests/diff_package_versions.rs`
/// works its totals out from: one file of each status, and small enough that
/// nothing here is near a size cap by accident.
fn diffable() -> Value {
    json!({
        "registry": "npm",
        "package": "diffable",
        "from_version": "1.0.0",
        "to_version": "2.0.0",
    })
}

/// The whole point of the cache, as an agent sees it.
///
/// The first call downloads two archives and compares them; the second is
/// handed what the first worked out. `cached` is how an agent tells the two
/// apart, and it is the only part of this an agent ever sees.
#[tokio::test]
async fn the_same_diff_asked_for_twice_is_remembered_the_second_time() {
    let store = Memory::new();

    let first = call(|| store.store(), diffable()).await;
    assert_eq!(
        first["structuredContent"]["cached"],
        json!(false),
        "nothing had been diffed yet, got {first}"
    );
    settles(&store, 2).await;

    let second = call(|| store.store(), diffable()).await;
    assert_eq!(
        second["structuredContent"]["cached"],
        json!(true),
        "the same comparison a second time is the one that was written, got \
         {second}"
    );
}

/// Remembering an answer is not allowed to change it.
///
/// A cache whose answers differ from the computation's is worse than no
/// cache: the difference is invisible to whoever asked and shows up later as
/// two agents disagreeing about the same comparison. So the whole answer is
/// compared and not a field of it — `cached` is the one field that is
/// *supposed* to differ, which is why it is the one taken out first.
///
/// What makes this hold is that what is remembered is the tree rather than
/// the answer: the totals and the sample are walked out of it on both paths,
/// so there is no second writer for them to disagree with.
#[tokio::test]
async fn a_remembered_answer_is_the_one_that_was_computed() {
    let store = Memory::new();

    let computed = call(|| store.store(), diffable()).await;
    settles(&store, 2).await;

    let remembered = call(|| store.store(), diffable()).await;

    assert_eq!(
        but_for_cached(remembered),
        but_for_cached(computed),
        "everything but `cached` is the same answer, the text block included"
    );
}

/// An entry is two blobs, under the name the comparison has.
///
/// The pathnames are the contract rather than an implementation detail: #27
/// computes the same `diff_id` in TypeScript and reads
/// `diffs/v1/{diff_id}/meta.json` with no server in the path, so a layout
/// this suite does not pin is a layout that can move without anything here
/// noticing.
///
/// Both of them, because half an entry is not a cache hit. A `meta.json`
/// written without its `patches.json` is a result whose patches are missing
/// with nothing saying so.
#[tokio::test]
async fn a_remembered_diff_is_two_blobs_under_the_name_the_comparison_has() {
    let store = Memory::new();

    let answer = call(|| store.store(), diffable()).await;
    settles(&store, 2).await;

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

/// What `patches.json` is for: every changed file, rendered once.
///
/// This is the whole reason the patches are written at all. Both archives are
/// extracted at this moment, so rendering every changed file costs almost
/// nothing; asking for one later costs two downloads (#15).
///
/// The expected patches are worked out from the fixture and from the
/// renderer's contract, not from running it — four shapes, one per case the
/// pair reaches:
///
/// - a file that is only in the second version is `/dev/null` against it,
///   every line prefixed `+`, and the trailing newline makes a last empty one
/// - a file that is only in the first is the mirror of that
/// - a file in both, changed, is the engine's unified diff
/// - a file in both whose content is identical is that content and
///   `is_diff: false`, so that a reader renders a file as a file — which is
///   what a rename with an unchanged body is
///
/// `README.md` is in neither version's patch set, because it did not change.
/// An unchanged file's patch is the file, and a cache that stored one would
/// be storing the package.
#[tokio::test]
async fn every_changed_file_is_remembered_with_its_patch_already_rendered() {
    let store = Memory::new();

    let answer = call(|| store.store(), diffable()).await;
    settles(&store, 2).await;

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

/// A patch too big to be worth keeping is left out, and the rest are kept.
///
/// The cap is a guard against pathological patch volume rather than a
/// property of a normal entry, so it is driven at a size the fixture can
/// reach instead of with a generated file: 100 bytes here, 256 KiB in
/// production, and the same code either way.
///
/// `src/index.js` has the only patch over it — it is the one file the pair
/// modifies, so its patch carries both versions of the line — and the three
/// smaller ones are still there. Omitting the whole entry because one file in
/// it was large would throw away the patches that fit, and #15 renders a
/// missing one on demand either way.
#[tokio::test]
async fn a_patch_over_the_cap_is_left_out_and_the_others_are_kept() {
    let store = Memory::new();

    let answer = call(|| store.store().capping_patches_at(100), diffable()).await;
    settles(&store, 2).await;

    let patches = blob(&store, &patches_of(&answer));

    assert_eq!(
        patches
            .as_object()
            .map(|patches| patches.keys().map(String::as_str).collect::<Vec<&str>>()),
        Some(vec!["src/added.js", "src/new-name.js", "src/removed.js"]),
        "the one patch over the cap is the only one missing, got {patches}"
    );
}

/// An entry too big to keep whole keeps its tree and says its patches are gone.
///
/// The tree is the expensive half to recompute and the small half to store,
/// so it stays; the patches are what a pathological comparison makes
/// enormous, and they go. `patches_omitted` is the difference between an
/// entry whose patches are missing and an entry with nothing to patch —
/// without it, #15 would have to guess which of the two it was looking at.
///
/// Driven at 100 bytes for the same reason the patch cap is, and shown
/// against a store with the real caps, because a flag that is always true
/// says nothing.
#[tokio::test]
async fn an_entry_over_the_cap_keeps_its_tree_and_says_its_patches_are_gone() {
    let store = Memory::new();

    let answer = call(|| store.store().capping_entries_at(100), diffable()).await;
    settles(&store, 1).await;

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
    settles(&whole, 2).await;

    assert_eq!(
        blob(&whole, &meta_of(&answer))["patches_omitted"],
        json!(false),
        "the same comparison under the real caps keeps them"
    );
}

/// An entry whose patches were dropped for their size is still an entry.
///
/// The cap takes the patches and keeps the tree, and the tree is what every
/// answer is walked out of — so the comparison is remembered and the second
/// call is a warm one. A `meta.json` with no `patches.json` beside it is a
/// whole entry when it says so, which is the whole of what `patches_omitted`
/// is for.
#[tokio::test]
async fn an_entry_that_dropped_its_patches_is_still_remembered() {
    let store = Memory::new();
    let capped = || store.store().capping_entries_at(100);

    call(capped, diffable()).await;
    settles(&store, 1).await;

    let second = call(capped, diffable()).await;

    assert_eq!(
        second["structuredContent"]["cached"],
        json!(true),
        "an entry that says its patches were dropped is a hit, got {second}"
    );
}

/// Half an entry is not a cache hit.
///
/// The two blobs are written one after the other, so a write that fails
/// between them leaves a `meta.json` whose `patches.json` never arrived. That
/// is not the absence above: this one says nothing was dropped, so answering
/// with it would serve a comparison whose every patch is silently missing —
/// a changed file that a reader of the entry would take for one with nothing
/// to render. Nothing reads an entry back yet; `get_file_diff` renders on
/// demand every time, so this is a wrong answer waiting rather than one
/// being given.
///
/// So it is a miss, and the miss is what repairs it: the recomputed entry
/// heads past the `meta.json` that is there and writes the blob that is not.
#[tokio::test]
async fn half_an_entry_is_not_a_cache_hit() {
    let store = Memory::new();

    let answer = call(|| store.store(), diffable()).await;
    settles(&store, 2).await;

    store.forget(&patches_of(&answer));

    let again = call(|| store.store(), diffable()).await;
    assert_eq!(
        again["structuredContent"]["cached"],
        json!(false),
        "an entry missing a half nothing said was dropped is not a hit, got \
         {again}"
    );

    settles(&store, 2).await;
    assert_eq!(
        store.written(),
        vec![meta_of(&answer), patches_of(&answer)],
        "and the miss writes back the blob that was gone"
    );
}

/// A store that is not there costs a recomputed diff and nothing else.
///
/// This is the rule the whole module is arranged around: the cache holds a
/// copy of an answer this server can work out again, so storage being down
/// degrades it to the server it was before #21 — correct, and slower. A
/// caller sees the same answer, and every call is a cold one.
///
/// The failure goes in the log instead, where an operator reads it. Nowhere
/// else is available to it: a `Failure` would reach the model, and the one
/// line a call leaves behind is the dispatch's and already written by the
/// time a backgrounded write has failed.
///
/// `DiffStore::unavailable` is not a mode invented for this test — it is
/// what `DiffStore::live` falls back to when a deployment has no credentials
/// to reach a store with.
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

/// The answer does not wait for the entry to be written.
///
/// A caller is waiting on a diff, not on a cache: the store holds a copy of
/// something this call has already worked out, so making the caller wait for
/// it to be filed is spending their latency on somebody else's next call.
///
/// Measured rather than assumed, which is what the store being slow is for.
/// A write that takes half a second and an answer that arrives in a fraction
/// of it is the only way to tell "written afterwards" from "written quickly"
/// — and the entry still lands, because a write nobody waits for is still a
/// write.
#[tokio::test]
async fn the_answer_does_not_wait_for_the_entry_to_be_written() {
    let store = Memory::new().stalling(Duration::from_millis(500));

    let began = Instant::now();
    let answer = call(|| store.store(), diffable()).await;
    let answered = began.elapsed();

    assert_eq!(
        answer["isError"],
        json!(false),
        "the comparison is the answer, got {answer}"
    );
    assert!(
        answered < Duration::from_millis(250),
        "the answer waited {answered:?} on a store that takes 500ms a blob"
    );

    settles(&store, 2).await;
    assert!(
        began.elapsed() >= Duration::from_millis(500),
        "a write this fast was not the slow store's, which would make the          measurement above meaningless"
    );
}

/// An entry that is already there is not written again.
///
/// Rewriting an identical entry would reset the moment it was uploaded, and
/// that moment is the order #22 evicts in — so a comparison that is asked
/// for often would keep moving to the back of the queue and the cache would
/// evict the entries that earn their place.
///
/// What makes a write happen at all when the entry is there is a read that
/// missed although it should not have: a transient failure on the lookup, or
/// two invocations computing the same comparison at once, neither able to
/// see what the other is about to write. A store whose reads are lost is
/// both of those, staged rather than raced.
#[tokio::test]
async fn an_entry_that_is_already_there_is_not_written_again() {
    let store = Memory::new();

    let answer = call(|| store.store(), diffable()).await;
    settles(&store, 2).await;

    let uploaded = |at: &str| {
        store
            .uploaded_at(at)
            .unwrap_or_else(|| panic!("`{at}` should be there"))
    };
    let (meta, patches) = (meta_of(&answer), patches_of(&answer));
    let (before_meta, before_patches) = (uploaded(&meta), uploaded(&patches));

    let missed = Memory::losing_reads(&store);
    let again = call(|| missed.store(), diffable()).await;

    assert_eq!(
        again["structuredContent"]["cached"],
        json!(false),
        "the call has to have missed, or there is no write to skip: got {again}"
    );

    // Long enough that a write would have landed: this store keeps its blobs
    // in a map, so an overwrite is microseconds away and not milliseconds.
    tokio::time::sleep(Duration::from_millis(100)).await;

    assert_eq!(
        (uploaded(&meta), uploaded(&patches)),
        (before_meta, before_patches),
        "the entry was written a second time, and eviction orders on this"
    );
}

/// A comparison is named by every argument, not by the package and the pair.
///
/// Each variation below changes one field of the cache key and nothing else,
/// and each has to be a miss with an entry of its own. A field left out of
/// the key would show here as a warm answer to a question nobody had asked —
/// the worst failure this cache has, because it is a confident wrong answer
/// rather than a slow one.
///
/// Every field a caller can send is varied but `registry`, which cannot be
/// varied on its own: the fixture set publishes `diffable` on npm and nowhere
/// else, so changing it alone is a package that is not there rather than a
/// second comparison. The schema number and the engine version are this
/// build's and cannot be reached from the wire at all. All three are held
/// against the golden vectors by `tests/cache_key.rs`.
#[tokio::test]
async fn changing_anything_the_comparison_is_named_by_is_an_entry_of_its_own() {
    let store = Memory::new();

    let first = call(|| store.store(), diffable()).await;
    settles(&store, 2).await;

    let named = |answer: &Value| answer["structuredContent"]["diff_id"].clone();

    // One field each, against the same baseline. `diffable` publishes two
    // versions, so a different `from` or `to` is one of them compared with
    // itself — a real comparison, and not the baseline's.
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
        settles(&store, 2 * (written + 1)).await;
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

// ---------------------------------------------------------------------------
// The budget
// ---------------------------------------------------------------------------

/// A store with room for one entry, asked to hold two, keeps the newer one.
///
/// The budget is derived from an entry this test has just written rather
/// than stated as a number, so it stays a budget with room for exactly one
/// of them when the fixture changes. What is asserted is not derived: the
/// two blobs that survive are named, and they are the second comparison's.
///
/// Driven at a few kilobytes for the reason the patch cap is driven at a
/// hundred bytes — a cap exercised with a small number and a real comparison
/// is the same code as one exercised with 256 MB and eight thousand of them.
#[tokio::test]
async fn a_budget_with_room_for_one_entry_keeps_the_newer_one() {
    let store = Memory::new();

    let first = call(|| store.store(), at(0.50)).await;
    lands(&store, &first).await;

    // One entry's worth, and the headroom production keeps over its target.
    let entry = held(&store);
    let budgeted = || store.store().budgeting(entry + entry / 16, entry);

    let second = call(budgeted, at(0.51)).await;
    lands(&store, &second).await;

    assert_eq!(
        store.written(),
        vec![meta_of(&second), patches_of(&second)],
        "the older comparison should have made way for the newer one"
    );
}

/// The arguments of the comparison every budget test writes, at `threshold`.
///
/// One comparison at a dozen thresholds rather than a dozen comparisons: the
/// threshold is a field of the cache key, so each is an entry of its own —
/// and `diffable`'s rename is of a file whose content did not change, so
/// every threshold below 1.0 detects it and every entry is the same size.
/// Entries that differ in size would make what a sweep deletes depend on
/// which of them it reached first.
fn at(threshold: f64) -> Value {
    let mut arguments = diffable();
    arguments["similarity_threshold"] = json!(threshold);
    arguments
}

/// How many bytes `store` is holding, summed from the store itself.
///
/// Read back through the blobs rather than taken from a total the sweep
/// worked out: the criterion is that the store stays inside its budget, and
/// a number eviction computed cannot disagree with eviction.
fn held(store: &Memory) -> u64 {
    store
        .written()
        .iter()
        .filter_map(|pathname| store.blob(pathname))
        .map(|bytes| bytes.len() as u64)
        .sum()
}

/// Wait until `store` holds the entry `answer` is about, or give up.
///
/// [`settles`] counts blobs, which a store that evicts no longer grows
/// monotonically: an entry admitted and then swept by the call after it
/// leaves the count where it was. So this waits for the entry itself.
async fn lands(store: &Memory, answer: &Value) {
    let (meta, patches) = (meta_of(answer), patches_of(answer));

    for _ in 0..400 {
        let written = store.written();
        if written.contains(&meta) && written.contains(&patches) {
            return;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }

    panic!(
        "the entry at `{meta}` should be there by now, {:?} is",
        store.written()
    );
}

/// Wait until `store` holds `blobs` of them, or give up.
///
/// A write happens after the answer, so "it was written" is something that
/// becomes true rather than something that is true when the answer arrives.
/// Polling is what a second caller does, and it is what the criterion means:
/// the entry lands, and the call did not wait for it.
async fn settles(store: &Memory, blobs: usize) {
    for _ in 0..400 {
        if store.written().len() >= blobs {
            return;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }

    panic!(
        "the store should hold {blobs} blobs by now, it holds {:?}",
        store.written()
    );
}

/// Where the patches of the comparison `answer` is about live.
fn patches_of(answer: &Value) -> String {
    format!("{}/patches.json", under(answer))
}

/// Where the tree of the comparison `answer` is about lives.
fn meta_of(answer: &Value) -> String {
    format!("{}/meta.json", under(answer))
}

/// The prefix the comparison `answer` is about is kept under.
fn under(answer: &Value) -> String {
    let diff_id = answer["structuredContent"]["diff_id"]
        .as_str()
        .unwrap_or_else(|| panic!("the answer names the comparison, got {answer}"));

    format!("diffs/v1/{diff_id}")
}

/// The blob at `pathname`, as the JSON it is.
fn blob(store: &Memory, pathname: &str) -> Value {
    let bytes = store
        .blob(pathname)
        .unwrap_or_else(|| panic!("`{pathname}` should be there, {:?} is", store.written()));

    serde_json::from_slice(&bytes)
        .unwrap_or_else(|e| panic!("`{pathname}` should be JSON, got {e}"))
}

/// One answer with `cached` taken out of both copies of it.
///
/// `tools::invoke` puts a tool's answer on the wire twice — as
/// `structuredContent` and as the text block `rmcp` mirrors it into — so a
/// field dropped from one is still in the other. The mirror is read back as
/// JSON rather than compared as a string, which is also what makes the
/// comparison above about the answer rather than about how it was spelled.
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

// ---------------------------------------------------------------------------
// Driving the wire
// ---------------------------------------------------------------------------

/// Call `diff_package_versions` with `arguments`, against a server whose
/// cache `store` builds.
///
/// A builder rather than a store, because a request gets its context from
/// the factory and a test makes several of them. It is where a test that
/// cares about a cap says so, and every other one says `|| store.store()`.
///
/// Panics with the JSON-RPC error rather than returning it: nothing in this
/// suite asks a question whose answer is a protocol error, so one arriving
/// means the call was built wrongly.
async fn call(store: impl Fn() -> DiffStore, arguments: Value) -> Value {
    let answer = post(
        store,
        json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "tools/call",
            "params": {
                "name": TOOL,
                "arguments": arguments,
                "_meta": {
                    "io.modelcontextprotocol/protocolVersion": CURRENT,
                    "io.modelcontextprotocol/clientCapabilities": {},
                },
            },
        }),
    )
    .await;

    if let Some(error) = answer.get("error") {
        panic!("expected a result, got JSON-RPC error {error}");
    }
    answer["result"].clone()
}

/// A request as a conforming `2026-07-28` client sends it, to a server whose
/// archives come from `fixtures/` and whose cache is `store`.
///
/// The context is built the way production builds one — through the service
/// factory — and then has its store replaced, which is the same `..self`
/// spread `Ctx::logging_to` is and not a builder that fills the seams it was
/// not given. Every other seam is still the fixture set's.
async fn post(store: impl Fn() -> DiffStore, body: Value) -> Value {
    let request = Request::builder()
        .method("POST")
        .uri("/mcp")
        .header("host", "mcp.diffpack.io")
        .header("accept", "application/json, text/event-stream")
        .header("content-type", "application/json")
        .header("mcp-protocol-version", CURRENT)
        .header("mcp-method", "tools/call")
        .header("mcp-name", TOOL)
        .body(Body::from(body.to_string()))
        .expect("the request should build");

    let ctx = Ctx::fixture(FIXTURES).storing_in(store());
    let router = router::router_with(move || Ok(Diffpack::with_ctx(ctx.clone())), Vec::new());

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
