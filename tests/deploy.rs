use std::path::Path;

fn repo_root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
}

fn vercel_json() -> serde_json::Value {
    let path = repo_root().join("vercel.json");
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("vercel.json should exist at {}: {e}", path.display()));
    serde_json::from_str(&text).expect("vercel.json should parse")
}

#[test]
fn the_function_vercel_json_configures_is_the_one_cargo_builds() {
    let manifest = vercel_json();

    let configured = manifest["functions"]
        .as_object()
        .expect("vercel.json should have a `functions` object")
        .keys()
        .next()
        .expect("`functions` should name the handler")
        .clone();

    assert!(
        repo_root().join(&configured).is_file(),
        "vercel.json configures `{configured}`, which is not a file in this repository"
    );

    let cargo: toml::Value =
        toml::from_str(include_str!("../Cargo.toml")).expect("Cargo.toml should parse");
    let built = cargo["bin"][0]["path"]
        .as_str()
        .expect("Cargo.toml should declare the handler as a [[bin]] with a path");

    assert_eq!(
        configured, built,
        "vercel.json configures `{configured}` but Cargo.toml builds `{built}`"
    );
}

#[test]
fn every_path_is_rewritten_to_the_handler() {
    let manifest = vercel_json();

    let rewrites = manifest["rewrites"]
        .as_array()
        .expect("vercel.json should have a `rewrites` array");

    let catch_all = rewrites
        .iter()
        .find(|r| r["source"] == "/(.*)")
        .expect("a `/(.*)` rewrite should send every path to the handler");

    let destination = catch_all["destination"]
        .as_str()
        .expect("the rewrite should have a destination");

    let handler = manifest["functions"]
        .as_object()
        .and_then(|f| f.keys().next())
        .expect("`functions` should name the handler");

    let expected = format!("/{}", handler.trim_end_matches(".rs"));

    assert_eq!(
        destination, expected,
        "the rewrite sends every path to `{destination}`, but the handler is deployed at `{expected}`"
    );
}
