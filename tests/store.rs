//! Cached diff results, driven the way an agent drives them.
//!
//! The cache has no surface of its own: no tool names it, and the only thing
//! an agent ever sees of it is that a comparison this server has already made
//! is cheap to ask for again. So every test here goes over the wire —
//! `tools/call` and `resources/read`, through the service factory
//! `router_with` takes — and the store under it is an in-process one the test
//! holds a handle to, the way `tests/log.rs` holds a
//! [`Capture`](diffpack_server::log::Capture).
//!
//! Four paths ask for a comparison and one of them writes what the others are
//! served, so a suite that only drove the writer could say what was put in the
//! store and nothing about what comes out of it. What says the difference is a
//! fixture set the comparison cannot be made through at all — see
//! [`ONE_SIDED`]: a call that answers through it did not fetch, which is a
//! stronger claim than a stopwatch and one a suite cannot get wrong.
//!
//! That handle is what lets a test say more than "it was cheap": it names the
//! blobs an entry is, and when each of them was written. Both are the
//! cache's contract rather than its internals — the pathnames are what #27
//! reads a result back from, the sizes are what the budget is counted in,
//! and the upload moment is the order eviction runs in.
//!
//! # What a test here is not
//!
//! A test of the Vercel Blob client. That is #20's, in `src/store/blob.rs`,
//! against a stub HTTP server, because the client is private to that module.
//! What is asserted here is the policy above it — which blobs an entry is,
//! when they are written, what happens when they are too big, what happens
//! when the store fails, and what happens when it is not there.
//!
//! That policy sits over one seam inside `src/store/`, five operations that
//! each answer or fail, and the store here and the real one are its two
//! adapters. A failure from either becomes a Note in one place above it, so
//! the store here can be told to fail any of the five
//! ([`Memory::failing`]) and what a test then watches is the line of policy
//! a real failure reaches. What is below the seam is not asserted here: the
//! client, which is #20's, and what only the real service can settle, which
//! is the one `#[ignore]`d test in `src/store/mod.rs`.

use std::time::{Duration, Instant};

mod common;

use common::{Client, FIXTURES};
use diffpack_server::log::Capture;
use diffpack_server::store::{DiffStore, Memory, Operation};
use diffpack_server::tools::Ctx;
use serde_json::{json, Value};

const TOOL: &str = "diff_package_versions";

/// A fixture set holding the first version of the pair below and not the
/// second.
///
/// What proves a reading path was served rather than recomputed. A count of
/// downloads or a stopwatch says a call was cheap; this says it did not
/// happen — through this set the second archive is a version the registry
/// does not publish, so a comparison that reaches the network through it
/// cannot be made at all, and a call that answers is a call that did not
/// reach.
///
/// `index.json` points the first version at the real set's tarball rather
/// than a copy of it, so there is one `diffable` 1.0.0 in this repository and
/// the two sets cannot come to disagree about what is in it.
const ONE_SIDED: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/fixtures/one-sided");

/// The JSON-RPC code a read gets when it reaches [`ONE_SIDED`] for the second
/// version and the registry publishes nothing there.
///
/// Spelled out rather than imported, because it is a wire contract and a
/// constant taken from the crate would agree with whatever the crate said.
/// `tests/resources.rs` is where this code is held to being a different one
/// from the code a URI that resolves to nothing gets; what it is for here is
/// that a read which had stopped resolving cannot pass as a fetch that
/// failed.
const NOT_PUBLISHED: i64 = -32001;

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

/// The same package compared with itself: a real comparison in which nothing
/// changed.
///
/// What a comparison with nothing to patch is, without a second fixture set
/// to hold one. Every file is `unchanged`, so no patch is rendered and none
/// goes missing — which is the side of `patches_omitted` that has to stay
/// `false`.
fn unchanged() -> Value {
    json!({
        "registry": "npm",
        "package": "diffable",
        "from_version": "1.0.0",
        "to_version": "1.0.0",
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

/// An entry that left one patch out says a patch was left out.
///
/// The other way patches go missing, and the one the flag used to be silent
/// about. A file whose patch was over the per-patch cap is absent from
/// `patches.json` — byte for byte what a file that did not change looks
/// like — so an entry declaring that nothing was dropped says the same thing
/// about the one file it cannot answer for as it says about the one there was
/// never anything to answer.
///
/// Read off the blob rather than off the wire, and not for convenience: a
/// reader that finds no patch for a file the tree says changed renders it
/// instead, whichever way the patch went missing, so no answer this server
/// gives differs on this flag. It is the entry's own record of what is in it,
/// and the entry is where it can be read.
#[tokio::test]
async fn an_entry_that_left_one_patch_out_says_a_patch_was_left_out() {
    let store = Memory::new();

    let answer = call(|| store.store().capping_patches_at(100), diffable()).await;
    settles(&store, 2).await;

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

/// A comparison with nothing to patch is not one whose patches were dropped.
///
/// The distinction the flag exists to draw, from the side that has to stay
/// `false`: a version compared with itself changes no file, so there is no
/// patch to render and none missing. An entry that said otherwise would send
/// a reader looking for something that was never there — and a flag that is
/// true whenever it is easier to say true says nothing at all.
#[tokio::test]
async fn a_comparison_with_nothing_to_patch_says_nothing_was_dropped() {
    let store = Memory::new();

    let answer = call(|| store.store(), unchanged()).await;
    settles(&store, 2).await;

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
/// with it would serve a comparison whose every patch is silently missing.
/// What that costs is not a wrong answer — a reader asks the comparison for
/// one file's patch and renders the file when there is none — but every one
/// of them: the two downloads this entry exists to save, on every call, for
/// as long as the half-written entry is there.
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

/// An entry that lost the patches it kept still answers for every file.
///
/// The one state the flag's correction moved out of the rule above, held
/// where it can be seen. An entry the per-patch cap trimmed says a patch is
/// missing from it *and* has a `patches.json`, so a lost one is read as the
/// absence the entry declared rather than as the half it is, and the entry is
/// served without the patches that did fit rather than being rewritten.
///
/// What that costs is those patches, until eviction takes the entry — the
/// two downloads this entry exists to save, on every call for one file's
/// diff, for as long as it is there. What it cannot cost is a wrong answer,
/// and that is the half worth pinning: a file absent from an entry is a file
/// to render, never a file reported unchanged, so the warm answer is the cold
/// one whichever way the patch went missing.
#[tokio::test]
async fn an_entry_that_lost_the_patches_it_kept_still_answers_for_every_file() {
    let store = Memory::new();
    let capped = || store.store().capping_patches_at(100);

    let answer = call(capped, diffable()).await;
    settles(&store, 2).await;

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

/// A read the store lost is a miss, and says the read is what failed.
///
/// The lookup is the half of the cache a caller waits on, and a lookup that
/// failed is answered the way one that found nothing is: the comparison is
/// worked out again. What tells the two apart is the Note, and an operator
/// reading the log needs it to be the same Note whether the real store lost
/// the read or the one this suite stages it in — a suite whose lost reads
/// were quiet would be a picture of a failed lookup quieter than
/// production's.
///
/// The entry is there, so the recomputed answer is the one that was
/// remembered, and the write that follows the miss finds it and skips.
#[tokio::test]
async fn a_lost_read_is_a_miss_that_says_the_read_failed() {
    let store = Memory::new();

    let answer = call(|| store.store(), diffable()).await;
    settles(&store, 2).await;

    let log = Capture::new();
    let lost = Memory::failing(&store, Operation::Read);
    let again = call(|| lost.store().logging_to(log.sink()), diffable()).await;
    let notes = noted(&log).await;

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
/// that moment is the order eviction runs in — so a comparison that is asked
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

    let missed = Memory::failing(&store, Operation::Read);
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
// What a reading path is served
// ---------------------------------------------------------------------------

/// A tool that reads a diff back is handed the stored tree rather than the
/// archives it was worked out from.
///
/// The half of the cache that was missing. `diff_package_versions` wrote the
/// entry and then only `diff_package_versions` read one, so the three paths
/// that exist to *read* a comparison each paid two archive downloads, an
/// extraction and a tree build for a tree the store already held.
///
/// Driven through [`ONE_SIDED`], so the claim is that nothing was fetched
/// rather than that fetching was quick. The control at the end is the same
/// call against a store with nothing in it: it has to reach the network, and
/// the network is where the second version is not.
#[tokio::test]
async fn a_reading_tool_is_served_the_stored_tree_rather_than_the_archives() {
    let store = Memory::new();

    let diffed = call(|| store.store(), diffable()).await;
    settles(&store, 2).await;
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

/// So is the resource that answers with the whole comparison.
///
/// A read is the other way an agent asks for a comparison it has already
/// made, and it was paying the same two downloads for the same tree. It goes
/// through the same walk as the tool now, which is what stops the two
/// disagreeing about which of them the cache is for.
///
/// A read has no `isError` to answer with, so served and missed are told
/// apart by the JSON-RPC envelope: a `result`, or an `error`. The miss names
/// its code as well as its presence — since #85 that code is the failure's
/// own, so "this fetch found nothing published" and "this URI is not ours"
/// are not one answer, and a read that had quietly stopped resolving could no
/// longer pass here as a fetch that failed.
#[tokio::test]
async fn a_read_of_the_whole_comparison_is_served_the_stored_tree_too() {
    let store = Memory::new();

    let diffed = call(|| store.store(), diffable()).await;
    settles(&store, 2).await;
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

/// One file's patch comes out of the entry too, archives and all.
///
/// The half of a cache hit `get_file_diff` could not spend. A tree holds
/// statuses, paths and line counts and never a file's contents, so an entry's
/// own patches are the only thing in it that can answer this tool — and they
/// have been rendered into every entry since #21 and read by nothing.
///
/// Driven through [`ONE_SIDED`] like the tree above: the second version is
/// not there to fetch, so a call that answers is a call that rendered
/// nothing. The control is the same call against the whole fixture set with
/// nothing cached, which is the patch this tool has always given.
#[tokio::test]
async fn a_file_diff_is_served_the_stored_patch_rather_than_the_archives() {
    let store = Memory::new();

    let diffed = call(|| store.store(), diffable()).await;
    settles(&store, 2).await;
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

/// A renamed file is served the patch that answers the question asked.
///
/// A patch is rendered from one path in each version, and an entry holds the
/// one the comparison's own tree names: `src/new-name.js` diffed from where
/// it was. A caller that leaves `old_path` out is asking about a file the
/// first version does not have, and this tool answers that with every line
/// added — a different answer, and one the entry has no patch for.
///
/// So the two are held apart here. Asked the way the tree tells a caller to
/// ask, the entry answers and nothing is fetched. Asked without it, the
/// warm call has to give what the cold one gives, which is the rule a cache
/// is only ever allowed to make faster.
#[tokio::test]
async fn a_renamed_file_is_served_only_the_patch_it_was_asked_for() {
    let store = Memory::new();

    let diffed = call(|| store.store(), diffable()).await;
    settles(&store, 2).await;
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

/// A directory is refused out of the entry, without the archives.
///
/// A directory has no patch, so an entry never holds one for it, and asking
/// the entry first answers nothing. What does answer is the tree: it says
/// `src` is a directory in both versions, which is the whole of what the
/// refusal needs. Fetching both archives to learn it again from their file
/// maps is two downloads spent on a question already answered.
///
/// Driven through [`ONE_SIDED`], so a call that downloaded would fail on the
/// second version. The served answer is held against the whole fixture set's
/// cold answer rather than against a sentence: the two are the same refusal,
/// and the control is the call that has to fetch and cannot.
#[tokio::test]
async fn a_directory_is_refused_out_of_the_entry_without_the_archives() {
    let store = Memory::new();

    let diffed = call(|| store.store(), diffable()).await;
    settles(&store, 2).await;
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

/// And whichever of a renamed file's two paths is named as where it was.
///
/// `diffable` renames `src/old-name.js` to `src/new-name.js`, and the tree
/// lists the file once, as renamed, at its new path. Neither path is a file
/// at `src` in either version, so asking for `src` with either one as its
/// `old_path` is still asking for a directory.
///
/// The new path is a file only in 2.0.0, so it says nothing about 1.0.0: the
/// tree has to read a renamed file as absent from the first version at the
/// path it is listed at, or it finds a file there, lets the call through and
/// fetches both archives to refuse it. The old path is a file in 1.0.0 that
/// the tree does not list there, so the tree refuses `src` without seeing
/// it — and so does a cold call, because the tree is asked first on both.
#[tokio::test]
async fn a_directory_is_refused_out_of_the_entry_whatever_old_path_names() {
    let store = Memory::new();

    let diffed = call(|| store.store(), diffable()).await;
    settles(&store, 2).await;

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

/// So is the resource that answers with one file's diff.
///
/// The document is the tool's answer with a media type on it (ADR 0014), so
/// a read that fetched where a call did not would be the two disagreeing
/// about what the cache is for. It takes the same two steps in the same
/// order: the entry's patch, and both archives only for a file the entry has
/// none for.
///
/// Served and missed are the envelope's two shapes, for the reason the whole
/// comparison's read above gives, and the miss names its code for the same
/// reason.
#[tokio::test]
async fn a_read_of_one_files_diff_is_served_the_stored_patch_too() {
    let store = Memory::new();

    let diffed = call(|| store.store(), diffable()).await;
    settles(&store, 2).await;
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

/// So is a read of a directory's diff.
///
/// The same question as the call above, asked through the resource, and it
/// has to cost the same: a read that downloaded two archives to refuse a
/// directory the tool refused without them would be the two disagreeing
/// about what the tree is for.
///
/// Held against the whole fixture set's cold read rather than against the
/// code, because the code does not tell the two failures apart: a directory
/// and a version the registry does not publish are both `-32001`. The
/// message beside the code does, and comparing two envelopes reads it without
/// pinning a sentence.
#[tokio::test]
async fn a_read_of_a_directory_is_refused_out_of_the_entry_too() {
    let store = Memory::new();

    let diffed = call(|| store.store(), diffable()).await;
    settles(&store, 2).await;
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

// ---------------------------------------------------------------------------
// One path, a file in one version and a directory in the other
// ---------------------------------------------------------------------------
//
// `lib` is a file in `shape` 1.0.0 and a directory in 2.0.0. The tree lists
// it twice, a file only 1.0.0 has and a directory only 2.0.0 has, side by
// side with one `path` (`diffpack-engine` 0.3.1; before it the engine kept one
// node per path and lost one of the two, philfreshman/diffpack-engine#7).
//
// A path is refused as a directory only when there is no file at it in either
// version (#111), so `lib` is served the file's patch. The four tests below
// are the four cells: each direction, cold and warm. A warm one is asked
// through `ONE_SIDED`, which has `shape` 1.0.0 and not 2.0.0, so it is also
// held to answering without a download.

/// A file that became a directory is served its removal when nothing is
/// stored.
///
/// The tree lists `lib` as a removed file, and that is the row an agent asks
/// about. The directory beside it in 2.0.0 does not stop the file being
/// diffed.
#[tokio::test]
async fn a_file_that_became_a_directory_is_served_cold() {
    let served = lib_cold("1.0.0", "2.0.0").await;

    is_the_patch_of_lib(&served, "--- from/lib\n+++ /dev/null\n");
}

/// And it is served the same patch when the comparison is stored.
///
/// The entry holds `lib`'s patch, so a warm call answers out of it and never
/// fetches 2.0.0, which this fixture set does not have.
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

/// A file where a directory was is served its addition when nothing is
/// stored.
///
/// The other way round: `lib` is a directory in 2.0.0, which is now the
/// version compared from. The tree lists a removed directory `lib` beside an
/// added file, and the file is what is diffed.
#[tokio::test]
async fn a_file_where_a_directory_was_is_served_cold() {
    let served = lib_cold("2.0.0", "1.0.0").await;

    is_the_patch_of_lib(&served, "--- /dev/null\n+++ to/lib\n");
}

/// And it is served the same patch when the comparison is stored.
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

/// The resource answers that path as the tool does, in all four cells.
///
/// A read of `diffpack://diff/{handle}/file/lib` is the tool's answer with a
/// media type on it (ADR 0014), so what is held is the document's text
/// against the tool's, and that the text is the file's patch.
///
/// A warm read and a warm call share one entry and go through `ONE_SIDED`
/// together, so neither can have downloaded.
#[tokio::test]
async fn a_read_of_a_path_that_is_a_file_and_a_directory_is_the_tools_answer() {
    for (from, to) in [("1.0.0", "2.0.0"), ("2.0.0", "1.0.0")] {
        let store = Memory::new();
        let diffed = call(|| store.store(), shape(from, to)).await;
        settles(&store, 2).await;
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
            assert_eq!(
                called["isError"],
                json!(false),
                "{from} → {to} {when}: `lib` is a file in 1.0.0, so it has a \
                 patch: got {called}"
            );
            assert_eq!(
                read["result"]["contents"][0]["text"], called["structuredContent"]["text"],
                "{from} → {to} {when}: the resource answers `lib` with the \
                 tool's patch, got {read} against {called}"
            );
        }
    }
}

/// The file inside that directory is served out of the entry.
///
/// `lib/index.js` is added in 2.0.0, so the entry holds its patch, and the
/// lookup for it has to go past the file `lib` beside the directory rather
/// than into it: a file has nothing under it. Asked through `ONE_SIDED`, a
/// lookup that went into the file and found nothing would have to fetch
/// 2.0.0 to render the patch, which this fixture set cannot do.
#[tokio::test]
async fn a_file_inside_a_path_that_became_a_directory_is_served_warm_without_the_archives() {
    let store = Memory::new();
    let diffed = call(|| store.store(), shape("1.0.0", "2.0.0")).await;
    settles(&store, 2).await;
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

/// An entry holds the patch for a file that became a directory.
///
/// The tree lists a removed file `lib`, and callers are served its patch, so
/// the comparison renders it while it has both archives in hand, like any
/// other changed file. `lib/index.js` inside the directory is added, and its
/// patch is kept too.
///
/// Read off the blob rather than off the wire: the warm tests above say an
/// answer came without a download, and the entry is where what it holds can
/// be read.
#[tokio::test]
async fn an_entry_holds_the_patch_for_a_file_that_became_a_directory() {
    let store = Memory::new();

    let answer = call(|| store.store(), shape("1.0.0", "2.0.0")).await;
    settles(&store, 2).await;

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

/// `shape` compared from `from` to `to`.
fn shape(from: &str, to: &str) -> Value {
    json!({
        "registry": "npm",
        "package": "shape",
        "from_version": from,
        "to_version": to,
    })
}

/// A handle for `shape` compared from `from` to `to`, minted in a store that
/// is dropped straight after.
async fn shape_handle(from: &str, to: &str) -> Value {
    let minted = Memory::new();
    let diffed = call(|| minted.store(), shape(from, to)).await;

    diffed["structuredContent"]["handle"].clone()
}

/// What `get_file_diff` answers for `lib` with nothing stored.
async fn lib_cold(from: &str, to: &str) -> Value {
    let asked = json!({ "handle": shape_handle(from, to).await, "path": "lib" });

    let cold = Memory::new();
    call_tool(|| cold.store(), "get_file_diff", asked).await
}

/// What `get_file_diff` answers for `lib` out of a stored comparison, through
/// a fixture set that cannot fetch 2.0.0.
async fn lib_warm(from: &str, to: &str) -> Value {
    let store = Memory::new();
    let diffed = call(|| store.store(), shape(from, to)).await;
    settles(&store, 2).await;
    let asked = json!({
        "handle": diffed["structuredContent"]["handle"].clone(),
        "path": "lib",
    });

    one_sided(|| store.store(), "get_file_diff", asked).await
}

/// Assert `answer` is `lib`'s patch, opening with `header`.
///
/// The header is the part worth holding: it says which way round the file
/// was diffed, removed from 1.0.0 or added in it.
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

/// A file the entry has no patch for is rendered, one file at a time.
///
/// Why the redeem asks for the file rather than trusting the entry as a
/// whole. `src/index.js` is the one patch the per-patch cap takes at a
/// hundred bytes; the three that fit are still in the entry. So one comes
/// out of the store and the other has to be fetched, out of the same entry
/// and in the same call — which is what "the patch this comparison holds for
/// this file" means and what a flag about the entry could not have said.
///
/// The same shape answers a patch dropped with the whole entry's, and an
/// entry written before there were patches to write, because none of the
/// three is a patch this comparison is holding.
#[tokio::test]
async fn a_file_whose_patch_was_dropped_is_rendered_rather_than_missed() {
    let store = Memory::new();
    let capped = || store.store().capping_patches_at(100);

    let diffed = call(capped, diffable()).await;
    settles(&store, 2).await;
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

/// The tool and the resource give one answer for every kind of file, warm
/// or cold.
///
/// The document is the tool's answer at its defaults with a media type on
/// it (ADR 0014), so the two are held against each other rather than each
/// against a literal: a literal would pass with the two disagreeing, as long
/// as each agreed with its own. Every way a file can reach an answer is
/// here — the patch an entry holds (`src/added.js`), the one the per-patch
/// cap took and has to be rendered (`src/index.js`), a file that did not
/// change (`README.md`), a renamed file (`src/new-name.js`) and a path in
/// neither version — and each is asked of an entry and of no entry at all.
///
/// A URI has room for one path, so the resource looks a renamed file's old
/// path up in the tree, and its answer is the tool's with `old_path` passed.
/// Without it the tool says every line was added, and it says so warm as it
/// does cold: a remembered patch is never the answer to a different pair of
/// paths.
#[tokio::test]
async fn the_tool_and_the_resource_answer_every_kind_of_file_alike() {
    let store = Memory::new();
    let capped = || store.store().capping_patches_at(100);

    let diffed = call(capped, diffable()).await;
    settles(&store, 2).await;
    let handle = diffed["structuredContent"]["handle"]
        .as_str()
        .unwrap_or_else(|| panic!("the answer carries a handle, got {diffed}"))
        .to_owned();

    // A store of its own for every call, so each cold one is: a shared one
    // would be written by the first call and serve the rest.
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

/// A store that is not there does not stop a reading path either.
///
/// The rule the whole module is arranged around, held over the three paths
/// that only just started asking: `DiffStore` has no `Result` to return, so a
/// lookup that could not be made is a miss and the recompute behind it is the
/// server that existed before there was a cache. What a caller sees is the
/// same page; what an operator sees is a note.
///
/// `DiffStore::unavailable` is not a mode invented for this test — it is what
/// `DiffStore::live` falls back to when a deployment has no credentials to
/// reach a store with.
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

    let notes = noted(&log).await;
    assert!(
        notes
            .iter()
            .all(|note| note.contains("blob store's credentials")),
        "what an operator reads is the store saying what was not there: got \
         {notes:?}"
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

    let first = call(|| store.store(), at(0)).await;
    lands(&store, &first).await;

    // One entry's worth, and the headroom production keeps over its target.
    let entry = held(&store);
    let budgeted = || store.store().budgeting(entry + entry / 16, entry);

    let second = call(budgeted, at(1)).await;
    lands(&store, &second).await;

    assert_eq!(
        store.written(),
        vec![meta_of(&second), patches_of(&second)],
        "the older comparison should have made way for the newer one"
    );
}

/// A run that writes many times the budget never takes the store past it.
///
/// The ceiling is the criterion the whole issue is written around, and it is
/// checked after every admission rather than once at the end: a store that
/// went over and came back under would pass a single check at the end and be
/// exactly the failure this is for.
///
/// Summed from the blobs the store holds rather than from a total the sweep
/// worked out. A number eviction computed cannot disagree with eviction, so
/// asserting against it would be asserting that the arithmetic is the
/// arithmetic.
#[tokio::test]
async fn a_run_well_past_the_budget_never_takes_the_store_over_it() {
    let store = Memory::new();

    let first = call(|| store.store(), at(0)).await;
    lands(&store, &first).await;

    // Room for three entries, and the headroom production keeps over its
    // target. Twelve are written into it.
    let entry = held(&store);
    let (max, target) = (3 * entry + entry / 16, 3 * entry);
    let budgeted = || store.store().budgeting(max, target);

    for step in 1..12 {
        let answer = call(budgeted, at(step)).await;
        lands(&store, &answer).await;

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

/// A sweep takes the oldest entries whole, and the newest survive it.
///
/// The store is seeded past the cap and what is left is named, which is
/// three things at once and deliberately one assertion. The survivors are
/// the newest two, so eviction is oldest-first; each of them is both of its
/// blobs, so an entry went or stayed whole; and nothing else is there, so
/// no `meta.json` was left without its `patches.json` or the reverse.
///
/// Insertion age and not least-recently-used: none of the five seeded
/// entries is read back before the sweep, so the order here is the order
/// they were written in and nothing else. That is what this issue asked
/// for, and #44 is why it is a latency cost rather than a tool going dark.
#[tokio::test]
async fn a_sweep_takes_the_oldest_entries_whole_and_the_newest_survive() {
    let store = Memory::new();

    let first = call(|| store.store(), at(0)).await;
    lands(&store, &first).await;
    let entry = held(&store);

    // Seeded past the cap: five entries, oldest first, under a budget with
    // room for all of them.
    let mut seeded = vec![first];
    for step in 1..5 {
        let answer = call(|| store.store(), at(step)).await;
        lands(&store, &answer).await;
        seeded.push(answer);
    }

    assert_eq!(
        held(&store),
        5 * entry,
        "the five weigh the same, which is what makes what a sweep deletes \
         predictable rather than a race between sizes"
    );

    // Room for two. Admitting a sixth has to take four.
    let budgeted = || store.store().budgeting(2 * entry + entry / 16, 2 * entry);
    let sixth = call(budgeted, at(5)).await;
    lands(&store, &sixth).await;

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

/// A put of an entry the store already holds evicts nothing.
///
/// Room is asked for before the write knows whether it has anything to
/// write, and a write skips a blob that is already at its pathname — so an
/// entry put a second time can sweep other comparisons out of a store to
/// make room it then never puts anything in. The cache is smaller afterwards
/// and holds the same entries it would have held anyway.
///
/// What makes a second put happen at all is a read that missed although it
/// should not have: a transient failure on the lookup, or two invocations
/// computing the same comparison at once. A store whose reads are lost is
/// both of those, staged rather than raced — the same seam
/// `an_entry_that_is_already_there_is_not_written_again` drives, under a
/// budget tight enough that a sweep would run.
#[tokio::test]
async fn a_put_of_an_entry_already_there_makes_no_room_it_will_not_use() {
    let store = Memory::new();

    let first = call(|| store.store(), at(0)).await;
    lands(&store, &first).await;
    let entry = held(&store);

    let second = call(|| store.store(), at(1)).await;
    lands(&store, &second).await;

    // Room for the two that are there and nothing more, so admitting
    // anything at all has to evict.
    let missed = Memory::failing(&store, Operation::Read);
    let budgeted = || missed.store().budgeting(2 * entry + entry / 16, 2 * entry);

    let again = call(budgeted, at(1)).await;
    assert_eq!(
        again["structuredContent"]["cached"],
        json!(false),
        "the call has to have missed, or there is no second put to make: got {again}"
    );

    // Long enough that a sweep would have finished: this store keeps its
    // blobs in a map, so a listing and a delete are microseconds away.
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

/// A put of the oldest entry does not sweep its own blobs away.
///
/// The sharp end of the same defect. The entry being put is in the listing a
/// sweep reads and may be the oldest thing in it, so the room it asks for
/// can be freed by deleting the very blobs the write is about to skip.
///
/// Against a store that forgets a blob the moment it is told to, the two
/// halves cancel: the delete lands, the write's head then misses, and both
/// blobs are written again. Against a real one they do not — Vercel Blob
/// takes up to a minute to propagate a delete, so the head can still see a
/// blob that is on its way out, skip the write, and leave the entry gone
/// once the delete arrives.
///
/// What is asserted here is the half this store can state, and it is the
/// half that decides the other: the entry's blobs were never touched. A
/// sweep that did not delete them has nothing to propagate and no window to
/// do it in. `uploaded_at` is what says so, and it is also the field
/// eviction orders on — rewriting the entry would move a comparison asked
/// for often to the back of that queue.
#[tokio::test]
async fn a_put_of_the_oldest_entry_does_not_sweep_itself_away() {
    let store = Memory::new();

    let oldest = call(|| store.store(), at(0)).await;
    lands(&store, &oldest).await;
    let entry = held(&store);

    let newer = call(|| store.store(), at(1)).await;
    lands(&store, &newer).await;

    let uploaded = |at: &str| {
        store
            .uploaded_at(at)
            .unwrap_or_else(|| panic!("`{at}` should be there, {:?} is", store.written()))
    };
    let (meta, patches) = (meta_of(&oldest), patches_of(&oldest));
    let before = (uploaded(&meta), uploaded(&patches));

    // Room for the two that are there and nothing more, and the entry put
    // again is the one a sweep would take first.
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

/// An entry that could never fit is refused before anything is deleted.
///
/// The sharpest way to lose a cache: an entry larger than the whole budget
/// arrives, a sweep runs to make room for it, every other comparison is
/// deleted, and there is still no room at the end of it. The store is then
/// empty and the entry that emptied it was never written either.
///
/// So the size is checked before the listing is taken, and what proves it is
/// the entry that was already there still being there. A refusal an operator
/// cannot see is a cache that has quietly stopped writing, so it says what it
/// would not take.
#[tokio::test]
async fn an_entry_too_big_for_the_whole_budget_is_refused_without_a_sweep() {
    let store = Memory::new();

    let first = call(|| store.store(), at(0)).await;
    lands(&store, &first).await;
    let entry = held(&store);

    // A budget no comparison this server makes could fit in.
    let log = Capture::new();
    let budgeted = || {
        store
            .store()
            .budgeting(entry / 2, entry / 4)
            .logging_to(log.sink())
    };

    let answered = call(budgeted, at(1)).await;
    let notes = noted(&log).await;

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

/// A sweep whose deletes all fail admits nothing.
///
/// The one thing about a sweep the wire cannot answer, and the reason the
/// store this suite writes to is allowed to refuse a delete at all: an
/// in-process map cannot fail to forget a blob, and a real store refuses
/// now and then. What the budget does with that refusal is the whole of the
/// ceiling — bytes credited to a delete that did not happen are room the
/// store does not have, and an entry admitted against them is the one
/// number this issue is written around, exceeded by the code that keeps it.
///
/// So the store here loses every delete. The sweep runs and frees nothing,
/// and what is left is exactly what was there before: the entry is refused
/// rather than written into room that was never made.
#[tokio::test]
async fn a_sweep_whose_deletes_all_fail_admits_nothing() {
    let store = Memory::new();

    let first = call(|| store.store(), at(0)).await;
    lands(&store, &first).await;
    let entry = held(&store);

    // Room for one entry, over a store that will not let go of anything.
    let log = Capture::new();
    let stubborn = Memory::failing(&store, Operation::Delete);
    let budgeted = || {
        stubborn
            .store()
            .budgeting(entry + entry / 16, entry)
            .logging_to(log.sink())
    };

    let second = call(budgeted, at(1)).await;
    let notes = noted(&log).await;
    stays_out(&store, &second).await;

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

/// A sweep that freed less than it needed admits nothing either.
///
/// The likelier half of the same failure, and the one that says the
/// arithmetic is per delete rather than per sweep: the store is smaller
/// afterwards and still has no room. A sweep that counted the entries it
/// tried would read this as room for the incoming one and be wrong by
/// everything the refused deletes weigh.
///
/// Five entries under a ceiling with space for three, so the sweep needs
/// three of them; it is given one. What survives is the four the store kept
/// — the single delete that worked took the oldest — and the comparison the
/// sweep was making room for is not among them.
#[tokio::test]
async fn a_sweep_that_freed_less_than_it_needed_admits_nothing() {
    let store = Memory::new();

    let first = call(|| store.store(), at(0)).await;
    lands(&store, &first).await;
    let entry = held(&store);

    let mut seeded = vec![first];
    for step in 1..5 {
        let answer = call(|| store.store(), at(step)).await;
        lands(&store, &answer).await;
        seeded.push(answer);
    }

    assert_eq!(
        held(&store),
        5 * entry,
        "the five weigh the same, which is what makes what a sweep frees a          number this test can state"
    );

    // Room for three of the five, swept down to two, over a store that takes
    // one delete and refuses every one after it.
    let log = Capture::new();
    let stubborn = Memory::failing_after(&store, Operation::Delete, 1);
    let budgeted = || {
        stubborn
            .store()
            .budgeting(3 * entry, 2 * entry)
            .logging_to(log.sink())
    };

    let sixth = call(budgeted, at(5)).await;
    let notes = noted(&log).await;
    stays_out(&store, &sixth).await;

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

/// A listing that could not be taken refuses the entry, and writes nothing.
///
/// The budget is read from the store on every write, because a total carried
/// in one invocation is a number two of them would disagree about. So a
/// listing that fails is a total that is not known — and admitting against a
/// total that is not known is how a ceiling gets exceeded. The cost of
/// refusing is one entry not cached, which is the cost of every other
/// failure the store has.
///
/// Under a budget with room for everything, so the listing is the only thing
/// that can refuse it. The diff is still the answer, and the refusal is a
/// Note saying which question went unanswered.
#[tokio::test]
async fn a_listing_that_fails_refuses_admission_and_writes_nothing() {
    let store = Memory::new();

    let log = Capture::new();
    let blind = Memory::failing(&store, Operation::List);
    let answer = call(|| blind.store().logging_to(log.sink()), diffable()).await;
    let notes = noted(&log).await;
    stays_out(&store, &answer).await;

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

/// A head that fails for one blob of an entry abandons the whole entry.
///
/// A head that could not be answered is not a blob that is missing, and
/// writing the rest of the entry around it is how a blob ends up without its
/// partner. A `patches.json` with no `meta.json` to name it is an orphan
/// nothing ever reads and the budget counts forever; a `meta.json` with no
/// `patches.json` is half an entry, read as a miss on every call until
/// something repairs it.
///
/// The store fails the head for `meta.json`, once, and answers every head
/// after it. So it says of `patches.json` that it is missing and worth
/// writing, and says the same of `meta.json` if it is asked again — which is
/// what a write that went ahead around the first failure would act on, and
/// the orphan it would leave behind.
#[tokio::test]
async fn a_head_that_fails_for_one_blob_of_an_entry_writes_neither_blob() {
    let store = Memory::new();

    let log = Capture::new();
    let unsure = Memory::failing_once(&store, Operation::Head);
    let answer = call(|| unsure.store().logging_to(log.sink()), diffable()).await;
    let notes = noted(&log).await;
    stays_out(&store, &answer).await;

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

/// A put that fails still answers the diff, and says it could not write.
///
/// The write happens after the answer has gone, so there is no caller left
/// for its failure to reach — and there should not be: the diff was worked
/// out, and a cache that could not keep a copy of it has cost one recomputed
/// diff next time and nothing else. What is left is a Note, because a write
/// that failed without saying so is a cache that has quietly stopped
/// working, which looks exactly like a cache that is working and cold.
#[tokio::test]
async fn a_put_that_fails_still_answers_the_diff_and_says_so() {
    let store = Memory::new();

    let log = Capture::new();
    let refusing = Memory::failing(&store, Operation::Write);
    let answer = call(|| refusing.store().logging_to(log.sink()), diffable()).await;
    let notes = noted(&log).await;

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

/// Fail if `store` ever writes the entry `answer` is about.
///
/// A refusal is an absence, and an absence is not something to wait for: it
/// is true when the call returns and stays true. What this waits out is the
/// other possibility — a store that was going to write after all and had not
/// reached it yet — so the window is far longer than the microseconds an
/// in-process write costs, and it is entered only once the sweep has already
/// said something. A failure here is the refusal not having happened, rather
/// than a machine being slow.
async fn stays_out(store: &Memory, answer: &Value) {
    let meta = meta_of(answer);
    let giving_up = Instant::now() + Duration::from_secs(1);

    while Instant::now() < giving_up {
        assert!(
            !store.written().contains(&meta),
            "the entry at `{meta}` was admitted into room the sweep never freed"
        );
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
}

/// Wait until `log` carries a note from the store, or give up.
///
/// A refusal happens where a write happens — after the answer — so it is
/// something that becomes true rather than something that is true when the
/// call returns.
async fn noted(log: &Capture) -> Vec<String> {
    for _ in 0..400 {
        let notes: Vec<String> = log
            .lines()
            .into_iter()
            .filter(|line| line.contains(r#""seam":"store""#))
            .collect();

        if !notes.is_empty() {
            return notes;
        }

        tokio::time::sleep(Duration::from_millis(5)).await;
    }

    panic!(
        "the store should have said something by now, {:?} is all there is",
        log.lines()
    );
}

/// Two admissions at once, each deciding against the other's absence.
///
/// This is what the gap between the ceiling and the target is for, and the
/// only test that can tell the two numbers apart: with a single number, a
/// sweep leaves the store exactly full, and the second admission — which
/// read the store before the first one's blobs arrived — finds room that is
/// already spoken for and takes it.
///
/// Staged rather than raced, the way a lost read stages two invocations
/// computing one comparison. The store takes a fifth of a second over every
/// blob, so the second call cannot help but list while the first call's
/// write is still in flight: that is the whole of the overlap, and without
/// it the two would simply happen in turn and agree.
#[tokio::test]
async fn two_admissions_at_once_do_not_take_the_store_over_the_ceiling() {
    let store = Memory::new();

    let first = call(|| store.store(), at(0)).await;
    lands(&store, &first).await;
    let entry = held(&store);

    for step in 1..4 {
        let answer = call(|| store.store(), at(step)).await;
        lands(&store, &answer).await;
    }

    // Four entries at the ceiling, a sweep down to three, so the gap is the
    // one entry a second admission needs — the proportion production keeps
    // between 256 MB and 240 MB over an entry that may weigh 8 MB.
    let (max, target) = (4 * entry, 3 * entry);
    let slow = store.clone().stalling(Duration::from_millis(200));
    let budgeted = || slow.store().budgeting(max, target);

    let (fifth, sixth) = tokio::join!(call(budgeted, at(4)), call(budgeted, at(5)));
    lands(&store, &fifth).await;
    lands(&store, &sixth).await;

    assert!(
        held(&store) <= max,
        "two entries admitted at once left the store holding {} bytes against \
         a ceiling of {max}",
        held(&store)
    );
}

/// The answer does not wait for the sweep, any more than for the write.
///
/// A caller is waiting on a diff and not on a cache, and a sweep is the most
/// expensive thing the cache does: a listing of the whole store and a delete
/// per entry it takes. Making a caller wait for it would spend their latency
/// on room for somebody else's next call.
///
/// Measured rather than assumed, in the three parts the write's own version
/// of this is measured in. The answer arrives in a fraction of what the
/// store takes over one blob; the entries the sweep is about to take are
/// still there when it does, which is what says the sweep had not run yet;
/// and they do go in the end, after more time has passed than the answer
/// took — because a sweep nobody waits for is still a sweep.
#[tokio::test]
async fn the_answer_does_not_wait_for_the_sweep() {
    let store = Memory::new();

    let oldest = call(|| store.store(), at(0)).await;
    lands(&store, &oldest).await;
    let entry = held(&store);

    let second = call(|| store.store(), at(1)).await;
    lands(&store, &second).await;

    // Room for one, so admitting a third has to take both of these, and a
    // store that takes half a second over every blob it touches.
    let slow = store.clone().stalling(Duration::from_millis(500));
    let budgeted = || slow.store().budgeting(entry + entry / 16, entry);

    let began = Instant::now();
    let third = call(budgeted, at(2)).await;
    let answered = began.elapsed();

    assert_eq!(
        third["isError"],
        json!(false),
        "the comparison is the answer, got {third}"
    );
    assert!(
        answered < Duration::from_millis(250),
        "the answer waited {answered:?} on a store that takes 500ms a blob"
    );
    assert!(
        store.written().contains(&meta_of(&oldest)),
        "the oldest entry was already gone when the answer arrived, so the \
         caller waited for the sweep after all"
    );

    lands(&store, &third).await;
    assert!(
        !store.written().contains(&meta_of(&oldest)),
        "the sweep nobody waited for should still have happened: {:?}",
        store.written()
    );
    assert!(
        began.elapsed() >= Duration::from_millis(500),
        "a sweep this fast was not the slow store's, which would make the \
         measurement above meaningless"
    );
}

/// A comparison the sweep took is recomputed and served, not refused.
///
/// This criterion was written when a bare `diff_id` was the only handle an
/// agent had, and it said an evicted entry produced an error telling the
/// agent to recompute. #44 changed what it means: the handle carries the
/// inputs beside the `diff_id`, so a reading tool whose entry is gone works
/// the comparison out again. Eviction stopped being something an agent sees.
///
/// Read before the sweep and after it, and the two answers compared, because
/// "served" is the claim and "the same thing" is what makes it worth
/// serving. It is true today because `get_diff_tree` recomputes on every
/// call; it has to stay true when that tool looks in the store first, and
/// this is where it would stop being true without anything else noticing.
#[tokio::test]
async fn a_comparison_the_sweep_took_is_recomputed_and_served() {
    let store = Memory::new();

    let diffed = call(|| store.store(), at(0)).await;
    lands(&store, &diffed).await;
    let entry = held(&store);

    let handle = diffed["structuredContent"]["handle"].clone();
    let walk = json!({ "handle": handle });
    let warm = call_tool(|| store.store(), "get_diff_tree", walk.clone()).await;

    // Room for one entry, so admitting the next takes this one.
    let budgeted = || store.store().budgeting(entry + entry / 16, entry);
    let next = call(budgeted, at(1)).await;
    lands(&store, &next).await;

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

/// The arguments of the `step`th comparison a budget test writes.
///
/// One comparison at many thresholds rather than many comparisons: the
/// threshold is a field of the cache key, so each is an entry of its own —
/// and `diffable`'s rename is of a file whose content did not change, so
/// every threshold below 1.0 detects it and every entry holds the same tree.
///
/// Never a round tenth, which is the whole of why this is a function and not
/// a literal. `meta.json` carries the threshold as the `f64` it is, and
/// `serde_json` writes `0.6` where it writes `0.61` — so an entry at a round
/// tenth is a byte lighter than its neighbours, and what a sweep deletes
/// would depend on a byte rather than on an age.
fn at(step: u32) -> Value {
    let hundredths = 51 + step + step / 9;

    let mut arguments = diffable();
    arguments["similarity_threshold"] = json!(f64::from(hundredths) / 100.0);
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
///
/// It waits far longer than [`settles`] does, and the number is deliberately
/// not a tight one. An entry that had to make room for itself costs a
/// listing, a delete per entry the sweep took and then its own two writes —
/// five blobs' worth of delay against a store a test has asked to take half
/// a second over each, where a plain write costs two. A bound near the real
/// figure is a test that passes on a quiet machine and fails on a busy one,
/// which is what this was before it did exactly that in CI.
///
/// Nothing is measured here. Every timing this suite asserts is taken before
/// this is called, so patience costs a slow failure and never a wrong pass.
async fn lands(store: &Memory, answer: &Value) {
    let (meta, patches) = (meta_of(answer), patches_of(answer));
    let giving_up = Instant::now() + Duration::from_secs(30);

    while Instant::now() < giving_up {
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
    call_tool(store, TOOL, arguments).await
}

/// Call `tool` with `arguments`, against a server whose cache `store` builds.
///
/// The cache is written by one tool and read back by the three that take a
/// handle, so a suite that could only drive the writer could not state what
/// happens to a reader when an entry is swept.
///
/// The context is built the way production builds one — through the service
/// factory the shared client hands `router_with` — and then has its store
/// replaced, which is the same `..self` spread `Ctx::logging_to` is and not a
/// builder that fills the seams it was not given. Every other seam is still
/// the fixture set's.
async fn call_tool(store: impl Fn() -> DiffStore, tool: &str, arguments: Value) -> Value {
    calling(FIXTURES, store, tool, arguments).await
}

/// The same, against a server that cannot fetch the second version.
///
/// See [`ONE_SIDED`]: everything but the archive the comparison needs is
/// there, so a call that answers through this is a call that was served.
async fn one_sided(store: impl Fn() -> DiffStore, tool: &str, arguments: Value) -> Value {
    calling(ONE_SIDED, store, tool, arguments).await
}

/// Call `tool` with `arguments`, against a server reading `fixtures` and
/// caching in `store`.
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

/// Read `uri`, against a server reading `fixtures` and caching in `store`.
///
/// The whole JSON-RPC envelope rather than the `result`, because what a read
/// that could not be made answers with is an `error` beside it — a resource
/// has no `isError` of its own to carry a failure in (#85).
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
