use base64::Engine as _;
use diffpack_server::cache_key::DiffKey;
use diffpack_server::engine;
use diffpack_server::handle::{DiffHandle, Inputs};
use diffpack_server::registry::Registry;
use serde_json::json;

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

fn zod_diff_id() -> String {
    let file: serde_json::Value =
        serde_json::from_str(include_str!("../fixtures/cache-key-vectors.json"))
            .expect("fixtures/cache-key-vectors.json should parse");
    let vectors = file["vectors"]
        .as_array()
        .expect("the file holds an array of vectors");

    let mut pinned = vectors
        .iter()
        .find(|vector| vector["name"] == "npm/zod")
        .expect("the worked example is a vector")["input"]
        .clone();
    pinned["engine"] = json!(engine::VERSION);

    vectors
        .iter()
        .find(|vector| vector["input"] == pinned)
        .unwrap_or_else(|| {
            panic!(
                "no vector holds the worked example at engine {}: add one to \
                 fixtures/cache-key-vectors.json, generated from docs/cache-key.md \
                 rather than from this crate",
                engine::VERSION
            )
        })["diff_id"]
        .as_str()
        .expect("a diff_id is a string")
        .to_owned()
}

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

#[test]
fn a_handle_names_the_diff_the_cache_key_document_names() {
    let minted = DiffHandle::mint(zod());

    assert_eq!(
        minted.diff_id(),
        zod_diff_id(),
        "the diff_id docs/cache-key.md's worked example produces"
    );
    assert_eq!(
        DiffHandle::decode(&minted.encode())
            .expect("a handle this server minted")
            .diff_id(),
        zod_diff_id(),
        "and the same one after a round trip"
    );
}

#[test]
fn a_handle_whose_diff_id_disagrees_with_its_inputs_is_refused() {
    let forged = handle_of(json!({
        "diff_id": zod_diff_id(),
        "schema": 1,
        "engine": engine::VERSION,
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

#[test]
fn a_handle_is_a_version_prefix_and_an_opaque_payload() {
    let encoded = DiffHandle::mint(zod()).encode();

    assert_eq!(
        encoded,
        handle_of(json!({
            "diff_id": zod_diff_id(),
            "schema": 1,
            "engine": engine::VERSION,
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

#[test]
fn a_malformed_handle_is_refused() {
    let payload = json!({
        "diff_id": zod_diff_id(),
        "schema": 1,
        "engine": engine::VERSION,
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
            zod_diff_id(),
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
            handle_of(json!({ "diff_id": zod_diff_id() })),
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

#[test]
fn an_argument_that_is_not_a_handle_is_refused_before_a_handler_runs() {
    let refused = serde_json::from_value::<DiffHandle>(json!(zod_diff_id()))
        .expect_err("a bare diff_id is not a handle");

    assert!(
        refused.to_string().contains("diff_package_versions"),
        "the message a tool's `-32602` carries should name the call that mints \
         a handle: {refused}"
    );
}

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

fn handle_of(payload: serde_json::Value) -> String {
    format!(
        "d1:{}",
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(payload.to_string())
    )
}
