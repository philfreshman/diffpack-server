mod common;

use common::Client;
use diffpack_server::registry::{self, Registry};
use serde_json::{json, Value};

const TOOLS: [&str; 8] = [
    "diff_package_versions",
    "get_diff_tree",
    "get_file_content",
    "get_file_diff",
    "list_package_files",
    "list_package_versions",
    "resolve_archive_url",
    "search_packages",
];

const COMPARISON_OPTIONS: [&str; 2] = ["similarity_threshold", "ignore_whitespace"];

const NEIGHBOURS: [(&str, &str); 5] = [
    ("get_file_content", "get_file_diff"),
    ("list_package_files", "get_diff_tree"),
    ("diff_package_versions", "get_diff_tree"),
    ("search_packages", "list_package_versions"),
    ("resolve_archive_url", "list_package_files"),
];

#[tokio::test]
async fn the_collection_is_the_tools_the_list_declares() {
    assert_eq!(
        names(&Client::fixture().tools().await),
        TOOLS,
        "a tool added to `tools!` is a tool this file holds to the rules \
         below, and these two lists are what say so"
    );
}

#[tokio::test]
async fn every_tool_is_listed_with_everything_an_agent_needs() {
    for tool in Client::fixture().tools().await {
        let name = named(&tool);

        assert!(
            tool["description"]
                .as_str()
                .is_some_and(|text| !text.is_empty()),
            "`{name}` is picked by an agent that has read nothing else, so it \
             needs a description: got {tool}"
        );

        assert_eq!(
            tool["inputSchema"]["type"], "object",
            "`{name}`'s input schema should be an object schema, got {}",
            tool["inputSchema"]
        );
        assert!(
            tool["inputSchema"]["properties"].is_object(),
            "`{name}`'s input schema should describe its arguments, got {}",
            tool["inputSchema"]
        );

        assert_eq!(
            tool["outputSchema"]["type"], "object",
            "`{name}` answers with structured content, so it has to declare \
             its shape: got {tool}"
        );
    }
}

#[tokio::test]
async fn every_tool_has_a_neighbour_an_agent_could_mistake_it_for() {
    for name in TOOLS {
        assert!(
            NEIGHBOURS
                .iter()
                .any(|(one, other)| [one, other].contains(&&name)),
            "`{name}` should be paired with the tool an agent would weigh it \
             against, so that its description is held to naming it"
        );
    }
}

#[tokio::test]
async fn every_tool_says_when_to_call_its_neighbour_instead() {
    let tools = Client::fixture().tools().await;
    let description = |name: &str| {
        tools
            .iter()
            .find(|tool| named(tool) == name)
            .and_then(|tool| tool["description"].as_str())
            .unwrap_or_default()
            .to_owned()
    };

    for (one, other) in NEIGHBOURS {
        for (tool, neighbour) in [(one, other), (other, one)] {
            assert!(
                description(tool).contains(&format!("`{neighbour}`")),
                "`{tool}` and `{neighbour}` are both plausible answers to \
                 one question, so `{tool}`'s description should name \
                 `{neighbour}` and say when to call it instead: got {}",
                description(tool)
            );
        }
    }
}

#[tokio::test]
async fn every_tool_declares_the_hints_a_client_decides_on() {
    for tool in Client::fixture().tools().await {
        let name = named(&tool);
        let annotations = &tool["annotations"];

        assert_eq!(
            annotations["readOnlyHint"], true,
            "`{name}` changes nothing — no tool here does — and a client \
             deciding whether to ask for confirmation reads this: got \
             {annotations}"
        );
        assert_eq!(
            annotations["openWorldHint"], true,
            "`{name}` answers about a package on a registry, which is a world \
             this server does not control, got {annotations}"
        );
        assert!(
            annotations["idempotentHint"].is_boolean(),
            "`{name}` has to say whether asking twice gives the same answer; \
             a default would be a guess: got {annotations}"
        );
    }
}

#[tokio::test]
async fn every_registry_argument_is_the_enum_the_registry_module_owns() {
    let mut asked = 0;

    for tool in Client::fixture().tools().await {
        let name = named(&tool);
        let registry = &tool["inputSchema"]["properties"]["registry"];
        if registry.is_null() {
            continue;
        }

        asked += 1;
        assert_eq!(
            registry["enum"],
            json!(["npm", "crates", "pypi"]),
            "`{name}` should list the registries this server has, got {registry}"
        );
    }

    assert!(
        asked > 0,
        "every tool but the ones that take a handle names a registry, so \
         finding none means this walked the wrong field"
    );
}

#[tokio::test]
async fn every_version_argument_states_the_version_rule() {
    let mut asked = 0;

    for tool in Client::fixture().tools().await {
        let name = named(&tool);
        for (field, schema) in arguments(&tool) {
            if field != "version" && !field.ends_with("_version") {
                continue;
            }

            asked += 1;
            let said = schema["description"].as_str().unwrap_or_default();
            assert!(
                said.contains(registry::VERSION_RULE),
                "`{name}`'s `{field}` should state the version rule, got {schema}"
            );
            assert!(
                said.contains("`v4.0.0` and `4.0.0` are different versions"),
                "`{name}`'s `{field}` should say that a `v` is part of a \
                 version, got {schema}"
            );
        }
    }

    assert!(
        asked >= 5,
        "four tools take a version and one takes two, so finding {asked} \
         means this walked the wrong field"
    );
}

#[tokio::test]
async fn every_package_argument_states_every_registrys_name_rule() {
    let client = Client::fixture();
    let listed = client
        .post(json!({ "jsonrpc": "2.0", "id": 1, "method": "resources/list" }))
        .await;
    let served: Vec<&str> = listed["result"]["resources"]
        .as_array()
        .map(|resources| {
            resources
                .iter()
                .filter_map(|resource| resource["uri"].as_str())
                .collect()
        })
        .unwrap_or_default();
    assert!(
        served.contains(&"diffpack://registries"),
        "the catalogue of registries should be listed, got {listed}"
    );

    let mut asked = 0;
    for tool in client.tools().await {
        let name = named(&tool);
        let Some(schema) = tool["inputSchema"]["properties"].get("package") else {
            continue;
        };

        asked += 1;
        let said = schema["description"].as_str().unwrap_or_default();
        for registry in Registry::ALL {
            assert!(
                said.contains(registry.name_rule()),
                "`{name}`'s `package` should state {}'s name rule, got {schema}",
                registry.name()
            );
        }
        assert!(
            said.contains("diffpack://registries"),
            "`{name}`'s `package` should say where the rules are served, got {schema}"
        );
    }

    assert!(
        asked >= 5,
        "five tools take a package, so finding {asked} means this walked the \
         wrong field"
    );
}

#[tokio::test]
async fn every_optional_argument_says_what_leaving_it_out_means() {
    let mut asked = 0;

    for tool in Client::fixture().tools().await {
        let name = named(&tool);
        let required = &tool["inputSchema"]["required"];
        for (field, schema) in arguments(&tool) {
            if required
                .as_array()
                .is_some_and(|fields| fields.contains(&json!(field)))
            {
                continue;
            }

            asked += 1;
            let said = schema["description"].as_str().unwrap_or_default();
            assert!(
                said.to_lowercase().contains("omit it"),
                "`{name}`'s `{field}` is optional, and an agent cannot see a \
                 default it is not told about: got {schema}"
            );

            let default = &schema["default"];
            if default.is_number() || default.is_boolean() {
                assert!(
                    states(said, &default.to_string()),
                    "`{name}`'s `{field}` defaults to {default}, which its \
                     description should say: got {schema}"
                );
            }
        }
    }

    assert!(
        asked >= 18,
        "seven tools take eighteen optional arguments between them, so \
         finding {asked} means this walked the wrong field"
    );
}

#[tokio::test]
async fn every_bounded_argument_says_where_its_bounds_are() {
    let mut asked = 0;

    for tool in Client::fixture().tools().await {
        let name = named(&tool);
        for (field, schema) in arguments(&tool) {
            let said = schema["description"].as_str().unwrap_or_default();
            for bound in bounds(schema) {
                asked += 1;
                assert!(
                    states(said, &bound),
                    "`{name}`'s `{field}` stops at {bound}, which its \
                     description should say: got {schema}"
                );
            }
        }
    }

    assert!(
        asked >= 16,
        "five kinds of argument carry sixteen bounds between them, so \
         finding {asked} means this walked the wrong field"
    );
}

#[tokio::test]
async fn a_comparisons_options_come_with_its_handle_and_are_not_asked_again() {
    let client = Client::fixture();
    let made = client
        .call(
            "diff_package_versions",
            json!({
                "registry": "npm",
                "package": "diffable",
                "from_version": "1.0.0",
                "to_version": "2.0.0",
            }),
        )
        .await;
    let handle = &made["structuredContent"]["handle"];
    let mut asked = 0;

    for tool in client.tools().await {
        let name = named(&tool);
        let Some(schema) = tool["inputSchema"]["properties"].get("handle") else {
            continue;
        };

        asked += 1;
        let said = schema["description"].as_str().unwrap_or_default();
        for option in COMPARISON_OPTIONS {
            assert!(
                said.contains(&format!("`{option}`")),
                "`{name}`'s `handle` should say that `{option}` is fixed by \
                 it, or an agent will try to pass it again: got {schema}"
            );

            let answer = client
                .respond(json!({
                    "jsonrpc": "2.0",
                    "id": 1,
                    "method": "tools/call",
                    "params": {
                        "name": name,
                        "arguments": { "handle": handle, "path": "index.js", option: 0.5 },
                    },
                }))
                .await
                .json();
            assert_eq!(
                answer["error"]["code"], -32602,
                "`{name}` should refuse `{option}` rather than ignore it, \
                 since the handle already fixed it: got {answer}"
            );
        }
    }

    assert_eq!(
        asked, 2,
        "two tools read a comparison back, so finding {asked} means this \
         walked the wrong field"
    );
}

#[tokio::test]
async fn no_description_an_agent_reads_names_a_rust_path() {
    let mut leaked = Vec::new();

    for tool in Client::fixture().tools().await {
        let name = named(&tool).to_owned();
        descriptions(&tool, &mut |said| {
            if said.contains("crate::") || said.contains("[`") {
                leaked.push(format!("{name}: {said}"));
            }
        });
    }

    assert!(
        leaked.is_empty(),
        "these reach a model and are written for us: {leaked:#?}"
    );
}

fn names(tools: &[Value]) -> Vec<&str> {
    tools.iter().map(named).collect()
}

fn named(tool: &Value) -> &str {
    tool["name"].as_str().unwrap_or("<unnamed>")
}

fn arguments(tool: &Value) -> Vec<(&str, &Value)> {
    tool["inputSchema"]["properties"]
        .as_object()
        .map(|properties| {
            properties
                .iter()
                .map(|(field, schema)| (field.as_str(), schema))
                .collect()
        })
        .unwrap_or_default()
}

fn bounds(schema: &Value) -> Vec<String> {
    let branches = ["oneOf", "anyOf"]
        .iter()
        .filter_map(|combinator| schema[combinator].as_array())
        .flatten();

    std::iter::once(schema)
        .chain(branches)
        .flat_map(|branch| [&branch["minimum"], &branch["maximum"]])
        .filter_map(Value::as_f64)
        .map(|bound| match bound.fract() == 0.0 {
            true => format!("{bound:.0}"),
            false => bound.to_string(),
        })
        .collect()
}

fn states(text: &str, value: &str) -> bool {
    let continues_it = |c: char| c.is_ascii_alphanumeric() || c == '.';

    text.match_indices(value).any(|(at, _)| {
        let before = text[..at].chars().next_back();
        let after = text[at + value.len()..].chars().next();
        let ends_a_sentence = after == Some('.')
            && text[at + value.len() + 1..]
                .chars()
                .next()
                .is_none_or(char::is_whitespace);

        !before.is_some_and(continues_it) && (ends_a_sentence || !after.is_some_and(continues_it))
    })
}

fn descriptions(value: &Value, found: &mut impl FnMut(&str)) {
    match value {
        Value::Object(fields) => {
            for (key, child) in fields {
                match (key.as_str(), child.as_str()) {
                    ("description", Some(said)) => found(said),
                    _ => descriptions(child, found),
                }
            }
        }
        Value::Array(items) => items.iter().for_each(|item| descriptions(item, found)),
        _ => {}
    }
}
