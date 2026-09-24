//! The diff handle: what it carries, and what it refuses.
//!
//! The handle is what passes between the tool that computes a diff (#13) and
//! the three that read one back (#14, #15, #16). ADR 0006 says why it carries
//! the inputs beside the `diff_id`; these tests are that ADR, written as
//! assertions.
//!
//! The expected `diff_id`s come from `fixtures/cache-key-vectors.json`, which
//! was generated from `docs/cache-key.md` rather than from this crate. A test
//! that minted a handle and asked the same code what its `diff_id` was would
//! agree with itself whatever either side did.

use base64::Engine as _;
use diffpack_server::cache_key::DiffKey;
use diffpack_server::handle::{DiffHandle, Inputs};
use diffpack_server::registry::Registry;
use serde_json::json;

/// The worked example in `docs/cache-key.md`, and the first vector in the
/// fixture: npm, `zod`, 3.25.76 → 4.0.0, at the default threshold.
fn zod() -> Inputs {
    Inputs {
        registry: Registry::Npm,
        package: "zod".to_owned(),
        from_version: "3.25.76".to_owned(),
        to_version: "4.0.0".to_owned(),
        similarity_threshold: 0.75,
        ignore_whitespace: false,
    }
}

/// That vector's `diff_id`, copied from the fixture.
const ZOD_DIFF_ID: &str = "282467a6bd7b210db4077e71bcf3901550b219852386085452f5739c6b6c436c";

/// The whole point of the handle. An entry the budget's sweep has taken is
/// gone, so what the reading tools have left is whatever the handle carried:
/// if every input survives the round trip, a miss costs a recomputation and
/// nothing else.
#[test]
fn a_handle_carries_every_input_a_recompute_needs() {
    let minted = DiffHandle::mint(zod());

    let decoded = DiffHandle::decode(&minted.encode()).expect("a handle this server minted");

    assert_eq!(
        decoded.inputs(),
        &zod(),
        "the registry, the package, both versions, the threshold and the \
         whitespace setting — everything #13 was given"
    );
}

/// And it still names the same diff. The `diff_id` is the cache lookup and
/// the string #27 needs; a handle that carried the inputs but pointed at a
/// different entry would be worse than one that carried nothing.
#[test]
fn a_handle_names_the_diff_the_cache_key_document_names() {
    let minted = DiffHandle::mint(zod());

    assert_eq!(
        minted.diff_id(),
        ZOD_DIFF_ID,
        "the diff_id docs/cache-key.md's worked example produces"
    );
    assert_eq!(
        DiffHandle::decode(&minted.encode())
            .expect("a handle this server minted")
            .diff_id(),
        ZOD_DIFF_ID,
        "and the same one after a round trip"
    );
}

/// The reason the handle is opaque and verified rather than a JSON object an
/// agent fills in: with the inputs in it, an agent that can read a field will
/// eventually edit one, and a handle whose `diff_id` no longer names the diff
/// its inputs describe is a bug with no good error message. So the two halves
/// are checked against each other, and a forged handle is the client's
/// mistake — `-32602`, on the channel a model never reads, because a model
/// did not write it.
#[test]
fn a_handle_whose_diff_id_disagrees_with_its_inputs_is_refused() {
    // zod's `diff_id`, beside somebody else's package.
    let forged = handle_of(json!({
        "diff_id": ZOD_DIFF_ID,
        "schema": 1,
        "engine": "0.3.0",
        "registry": "npm",
        "package": "left-pad",
        "from": "3.25.76",
        "to": "4.0.0",
        "similarity_threshold": 0.75,
        "ignore_whitespace": false,
    }));

    let error = DiffHandle::decode(&forged)
        .expect_err("a handle this server did not mint")
        .respond()
        .expect_err("the client's mistake, not the model's");

    assert_eq!(error.code.0, -32602);
    assert!(
        error.message.contains("diff_package_versions"),
        "the message should name where a handle comes from, not just refuse: {}",
        error.message
    );
}

/// The wire format, written out here rather than read back from the encoder:
/// a version prefix, and one base64url payload with nothing in it a client is
/// invited to edit. The prefix is what lets a second format arrive later
/// without a handle from this one being mistaken for it.
#[test]
fn a_handle_is_a_version_prefix_and_an_opaque_payload() {
    let encoded = DiffHandle::mint(zod()).encode();

    assert_eq!(
        encoded,
        handle_of(json!({
            "diff_id": ZOD_DIFF_ID,
            "schema": 1,
            "engine": "0.3.0",
            "registry": "npm",
            "package": "zod",
            "from": "3.25.76",
            "to": "4.0.0",
            "similarity_threshold": 0.75,
            "ignore_whitespace": false,
        })),
        "the `diff_id` and the whole key it was computed from"
    );
    assert!(
        !encoded.contains("zod"),
        "a payload an agent can read is a payload an agent will edit: {encoded}"
    );
}

/// Every way a string can fail to be a handle takes the same channel and
/// says the same thing, because the fix is the same call. `-32602` and not a
/// tool error: a model did not write any of these.
#[test]
fn a_malformed_handle_is_refused() {
    let payload = json!({
        "diff_id": ZOD_DIFF_ID,
        "schema": 1,
        "engine": "0.3.0",
        "registry": "npm",
        "package": "zod",
        "from": "3.25.76",
        "to": "4.0.0",
        "similarity_threshold": 0.75,
        "ignore_whitespace": false,
    });

    let mut extra_field = payload.clone();
    extra_field["nonce"] = json!("1");

    let mut unknown_registry = payload.clone();
    unknown_registry["registry"] = json!("go");

    for (what, text) in [
        ("nothing at all", String::new()),
        (
            "a bare diff_id, which is what #14 used to take",
            ZOD_DIFF_ID.to_owned(),
        ),
        ("a version prefix this format does not have", {
            let encoded = DiffHandle::mint(zod()).encode();
            encoded.replace("d1:", "d2:")
        }),
        (
            "a payload that is not base64url",
            "d1:not base64!".to_owned(),
        ),
        (
            "base64url that is not JSON",
            format!(
                "d1:{}",
                base64::engine::general_purpose::URL_SAFE_NO_PAD.encode("{"),
            ),
        ),
        (
            "a payload missing a field",
            handle_of(json!({ "diff_id": ZOD_DIFF_ID })),
        ),
        (
            "a payload with a field this build does not know",
            handle_of(extra_field),
        ),
        (
            "a registry this server does not have",
            handle_of(unknown_registry),
        ),
    ] {
        let refused = match DiffHandle::decode(&text) {
            Ok(decoded) => panic!("{what} decoded, to {decoded:?}"),
            Err(refused) => refused,
        };
        let error = match refused.respond() {
            Ok(answer) => panic!("{what} reached the model as {answer:?}"),
            Err(error) => error,
        };

        assert_eq!(error.code.0, -32602, "{what}");
        assert!(
            error.message.contains("diff_package_versions"),
            "{what}: the message should name the call that mints one: {}",
            error.message,
        );
    }
}

/// A handle minted before an engine bump is internally consistent — its
/// `diff_id` is the hash of the inputs beside it — and still not one this
/// build can act on: the entry it names was computed by code that is no
/// longer here, and recomputing would write today's diff under yesterday's
/// key. It is refused, and the refusal says which of the two problems it is
/// rather than leaving a stale handle looking like a forged one.
#[test]
fn a_handle_from_another_build_of_this_server_is_refused() {
    let older = DiffKey {
        schema: 1,
        engine: "0.2.0".to_owned(),
        registry: "npm".to_owned(),
        package: "zod".to_owned(),
        from: "3.25.76".to_owned(),
        to: "4.0.0".to_owned(),
        similarity_threshold: 0.75,
        ignore_whitespace: false,
    };
    let stale = handle_of(json!({
        "diff_id": older.diff_id(),
        "schema": older.schema,
        "engine": older.engine,
        "registry": older.registry,
        "package": older.package,
        "from": older.from,
        "to": older.to,
        "similarity_threshold": older.similarity_threshold,
        "ignore_whitespace": older.ignore_whitespace,
    }));

    let error = DiffHandle::decode(&stale)
        .expect_err("a handle from a build that is not this one")
        .respond()
        .expect_err("the client's mistake, not the model's");

    assert_eq!(error.code.0, -32602);
    assert!(
        error.message.contains("0.2.0"),
        "the message should name the build that minted it: {}",
        error.message,
    );
}

/// What #13 puts in its answer and what #14, #15 and #16 read out of their
/// arguments: one string, not an object with six fields a client fills in.
/// The `diff_id` still travels beside it as a plain string, because #27 looks
/// a result up by exactly that.
#[test]
fn a_handle_is_one_string_on_the_wire() {
    let handle = DiffHandle::mint(zod());

    assert_eq!(
        serde_json::to_value(&handle).expect("a handle serialises"),
        json!(handle.encode()),
        "an answer carries the encoded handle and not its parts"
    );
    assert_eq!(
        serde_json::from_value::<DiffHandle>(json!(handle.encode()))
            .expect("an argument this server minted"),
        handle,
        "and an argument is read back into the handle it was"
    );
}

/// A handle that does not survive `decode` does not survive deserialisation
/// either, which is what makes a tool's `-32602` automatic: `tools::invoke`
/// turns a rejected argument into `InvalidParams` before a handler runs, so
/// no handler has to remember to verify one.
#[test]
fn an_argument_that_is_not_a_handle_is_refused_before_a_handler_runs() {
    let refused = serde_json::from_value::<DiffHandle>(json!(ZOD_DIFF_ID))
        .expect_err("a bare diff_id is not a handle");

    assert!(
        refused.to_string().contains("diff_package_versions"),
        "the message a tool's `-32602` carries should name the call that mints \
         a handle: {refused}"
    );
}

/// And what a client is shown for it: a string, described where the field is,
/// saying where a handle comes from and that it is not built by hand (#23).
#[test]
fn the_schema_says_a_handle_is_minted_rather_than_written() {
    let schema = serde_json::to_value(schemars::schema_for!(DiffHandle)).expect("a schema is JSON");

    assert_eq!(
        schema["type"], "string",
        "a handle is one string on the wire, got {schema}"
    );

    let description = schema["description"]
        .as_str()
        .unwrap_or_else(|| panic!("a described field, got {schema}"));
    assert!(
        description.contains("diff_package_versions"),
        "the description should name where a handle comes from: {description}"
    );
    assert!(
        description.contains("not"),
        "and say it is not written by hand: {description}"
    );
}

/// A handle built from `payload`, encoded the way the format says rather than
/// the way this crate happens to encode one.
fn handle_of(payload: serde_json::Value) -> String {
    format!(
        "d1:{}",
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(payload.to_string())
    )
}
