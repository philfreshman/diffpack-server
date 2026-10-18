mod common;

use std::collections::BTreeSet;

use common::{Client, PREVIOUS};
use diffpack_server::registry::{Registry, VERSION_RULE};
use serde_json::{json, Map, Value};

const REVIEW_UPGRADE: &str = "review_upgrade";

const ARGUMENTS: [&str; 4] = ["registry", "package", "from_version", "to_version"];

#[tokio::test]
async fn a_client_is_told_this_server_has_prompts() {
    let answer = current(json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "server/discover",
        "params": {},
    }))
    .await;

    assert!(
        answer["result"]["capabilities"]["prompts"].is_object(),
        "a server offering prompts should say so where a client looks, got {}",
        answer["result"]["capabilities"]
    );
}

#[tokio::test]
async fn review_upgrade_is_listed_with_a_title_and_a_description() {
    let prompt = review_upgrade().await;

    assert!(
        prompt["title"]
            .as_str()
            .is_some_and(|title| !title.is_empty()),
        "a client shows a prompt by its title, got {prompt}"
    );
    assert!(
        prompt["description"]
            .as_str()
            .is_some_and(|description| !description.is_empty()),
        "a person picks a prompt by its description, got {prompt}"
    );
}

#[tokio::test]
async fn review_upgrade_takes_its_four_arguments_in_the_order_they_are_said() {
    let prompt = review_upgrade().await;

    assert_eq!(
        argument_names(&prompt),
        ARGUMENTS,
        "a client that fills arguments by position fills them in this order, \
         so it is the order a person says them in: got {prompt}"
    );
}

#[tokio::test]
async fn every_argument_is_required_and_described() {
    for argument in arguments(&review_upgrade().await) {
        let name = &argument["name"];

        assert_eq!(
            argument["required"], true,
            "`{name}` has no default, so it has to be asked for: got {argument}"
        );
        assert!(
            argument["description"]
                .as_str()
                .is_some_and(|description| !description.is_empty()),
            "`{name}` is filled in by a person reading nothing but this, got {argument}"
        );
    }
}

#[tokio::test]
async fn the_registry_argument_names_every_registry_by_the_identifier_it_takes() {
    let prompt = review_upgrade().await;
    let registry = described(&prompt, "registry");

    for known in Registry::ALL {
        assert!(
            registry.contains(&format!("`{}`", known.id())),
            "a prompt argument has no enum, so its description is the only \
             place `{}` can be said: got {registry:?}",
            known.id()
        );
    }
}

#[tokio::test]
async fn the_version_arguments_say_which_is_which_and_state_the_version_rule() {
    let prompt = review_upgrade().await;

    for (field, side) in [("from_version", "from"), ("to_version", "to")] {
        let said = described(&prompt, field);

        assert!(
            said.contains(&format!("upgrading {side}")),
            "`{field}` should say it is the version being upgraded {side}, \
             because swapped they read the diff backwards: got {said:?}"
        );
        assert!(
            said.contains(VERSION_RULE),
            "`{field}` should state the version rule, got {said:?}"
        );
    }
}

#[tokio::test]
async fn prompts_list_carries_a_ttl_and_a_cache_scope() {
    let answer = listing().await;
    let result = &answer["result"];

    assert!(
        result["ttlMs"].as_u64().is_some_and(|ttl| ttl > 0),
        "prompts/list should say how long its answer stays fresh, got {result}"
    );
    assert_eq!(
        result["cacheScope"], "public",
        "the prompt list is the same for every caller, got {result}"
    );
}

#[tokio::test]
async fn prompts_are_listed_by_name() {
    let names: Vec<String> = prompts()
        .await
        .iter()
        .map(|prompt| named(prompt).to_owned())
        .collect();

    let mut sorted = names.clone();
    sorted.sort();

    assert_eq!(
        names, sorted,
        "prompts/list should be in one order, by name, as tools/list is"
    );
}

#[tokio::test]
async fn the_review_names_the_package_the_registry_and_both_versions() {
    let text = review("npm", "zod", "3.25.76", "4.0.0").await;

    for named in ["`zod`", "npm", "3.25.76", "4.0.0"] {
        assert!(
            text.contains(named),
            "the review should be about the package asked for, so it should \
             name {named}: got {text}"
        );
    }
}

#[tokio::test]
async fn the_review_says_a_registry_the_way_the_registry_says_itself() {
    let text = review("crates", "serde", "1.0.200", "1.0.210").await;

    assert!(
        text.contains(Registry::Crates.name()),
        "a person reads `crates.io`, not the identifier a tool takes: got {text}"
    );
}

#[tokio::test]
async fn the_review_hands_over_the_exact_arguments_to_compare_with() {
    let text = review("npm", "@types/node", "20.0.0", "22.0.0").await;

    let arguments = json!({
        "registry": "npm",
        "package": "@types/node",
        "from_version": "20.0.0",
        "to_version": "22.0.0",
    });

    assert!(
        text.contains(&arguments.to_string()),
        "an agent copies arguments it is handed rather than re-spelling a \
         scoped name, so they should be in the text as JSON: got {text}"
    );
}

#[tokio::test]
async fn the_review_walks_through_the_comparison_in_order() {
    let text = review("npm", "zod", "3.25.76", "4.0.0").await;

    let steps = [
        "`diff_package_versions`",
        "`get_diff_tree`",
        "`get_file_diff`",
    ];
    let at: Vec<usize> = steps
        .iter()
        .map(|step| {
            text.find(step)
                .unwrap_or_else(|| panic!("the review should name {step}, got {text}"))
        })
        .collect();

    let mut ordered = at.clone();
    ordered.sort_unstable();
    assert_eq!(
        at, ordered,
        "the tools should come in the order they are called — compare, list \
         what changed, read the diffs — got {text}"
    );
}

#[tokio::test]
async fn the_review_says_what_an_agent_gets_wrong_unprompted() {
    let text = review("npm", "zod", "3.25.76", "4.0.0").await;

    assert!(
        text.contains("not a removal and an addition"),
        "a renamed file is one file that moved, and an agent left to itself \
         reports it as two: got {text}"
    );
    assert!(
        text.contains("A large `unchanged` count is normal"),
        "most of a package does not change between two versions, and an \
         agent left to itself reports that it did not: got {text}"
    );
}

#[tokio::test]
async fn the_review_asks_for_a_report_about_what_could_break_for_a_consumer() {
    let text = review("npm", "zod", "3.25.76", "4.0.0").await;

    for asked in [
        "removed or renamed exports",
        "changed function, method or type signatures",
        "new required options",
        "dependency changes",
    ] {
        assert!(
            text.contains(asked),
            "the report is about what breaks for a consumer, so it should ask \
             about {asked}: got {text}"
        );
    }
}

#[tokio::test]
async fn the_review_points_at_the_file_each_registry_declares_dependencies_in() {
    for (registry, manifest) in [
        ("npm", "`package.json`"),
        ("crates", "`Cargo.toml`"),
        ("pypi", "`PKG-INFO`"),
    ] {
        let text = review(registry, "anything", "1.0.0", "2.0.0").await;

        assert!(
            text.contains(manifest),
            "dependency changes are read in {manifest} on {registry}, so the \
             review should send an agent there: got {text}"
        );
    }
}

#[tokio::test]
async fn every_tool_and_field_the_review_names_is_one_this_server_has() {
    let text = review("npm", "zod", "3.25.76", "4.0.0").await;
    let surface = tool_surface().await;

    let named = field_shaped_names_in(&text);
    assert!(
        named.len() >= 5,
        "the review names tools and their fields, so finding {named:?} means \
         this read the wrong text"
    );

    for name in named {
        assert!(
            surface.contains(&name),
            "the review names `{name}`, which no tool here has as its name or \
             as one of its fields; a prompt confidently sending an agent to a \
             tool that was renamed is worse than no prompt"
        );
    }
}

#[tokio::test]
async fn the_review_is_one_message_from_the_user() {
    let answer = getting(review_arguments("npm", "zod", "3.25.76", "4.0.0")).await;
    let messages = messages(&answer);

    assert_eq!(messages.len(), 1, "got {answer}");
    assert_eq!(messages[0]["role"], "user", "got {answer}");
    assert_eq!(messages[0]["content"]["type"], "text", "got {answer}");
}

#[tokio::test]
async fn a_missing_argument_is_invalid_params_and_named() {
    for missing in ARGUMENTS {
        let mut arguments = review_arguments("npm", "zod", "3.25.76", "4.0.0");
        arguments.remove(missing);

        let answer = getting(arguments).await;

        assert_eq!(
            answer["error"]["code"], -32602,
            "without `{missing}` there is no review to give: got {answer}"
        );
        assert!(
            answer["error"]["message"]
                .as_str()
                .is_some_and(|message| message.contains(missing)),
            "the refusal should say which argument is missing, got {answer}"
        );
    }
}

#[tokio::test]
async fn an_empty_argument_is_invalid_params_and_named() {
    for empty in ARGUMENTS {
        let mut arguments = review_arguments("npm", "zod", "3.25.76", "4.0.0");
        arguments.insert(empty.to_owned(), json!(""));

        let answer = getting(arguments).await;

        assert_eq!(
            answer["error"]["code"], -32602,
            "a client that fills arguments by position sends what was not \
             typed as an empty `{empty}`, which is as missing as an absent \
             one: got {answer}"
        );
        assert!(
            answer["error"]["message"]
                .as_str()
                .is_some_and(|message| message.contains(empty)),
            "the refusal should say which argument is empty, got {answer}"
        );
    }
}

#[tokio::test]
async fn a_registry_this_server_does_not_have_is_invalid_params() {
    let answer = getting(review_arguments("rubygems", "rails", "7.0.0", "7.1.0")).await;

    assert_eq!(answer["error"]["code"], -32602, "got {answer}");
    assert!(
        answer["error"]["message"]
            .as_str()
            .is_some_and(|message| message.contains("npm, crates, pypi")),
        "the refusal should name the registries there are, got {answer}"
    );
}

#[tokio::test]
async fn an_argument_the_prompt_does_not_take_is_invalid_params() {
    let mut arguments = review_arguments("npm", "zod", "3.25.76", "4.0.0");
    arguments.insert("from".to_owned(), json!("3.25.76"));

    let answer = getting(arguments).await;

    assert_eq!(
        answer["error"]["code"], -32602,
        "a misspelt argument is refused rather than ignored, as a tool's is: \
         got {answer}"
    );
}

#[tokio::test]
async fn a_prompt_this_server_does_not_have_is_invalid_params() {
    let answer = current(json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "prompts/get",
        "params": { "name": "review_everything", "arguments": {} },
    }))
    .await;

    assert_eq!(answer["error"]["code"], -32602, "got {answer}");
    assert!(
        answer["error"]["message"]
            .as_str()
            .is_some_and(|message| message.contains("review_everything")),
        "the refusal should name the prompt asked for, got {answer}"
    );
}

#[tokio::test]
async fn a_previous_revision_client_lists_and_gets_a_prompt() {
    let listed = previous(json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "prompts/list",
        "params": {},
    }))
    .await;

    assert_eq!(
        listed["result"]["prompts"][0]["name"], REVIEW_UPGRADE,
        "got {listed}"
    );

    let got = previous(json!({
        "jsonrpc": "2.0",
        "id": 2,
        "method": "prompts/get",
        "params": {
            "name": REVIEW_UPGRADE,
            "arguments": review_arguments("npm", "zod", "3.25.76", "4.0.0"),
        },
    }))
    .await;

    assert!(
        got["result"]["messages"][0]["content"]["text"].is_string(),
        "a client on the previous revision should get a prompt too, got {got}"
    );
}

async fn review(registry: &str, package: &str, from: &str, to: &str) -> String {
    let answer = getting(review_arguments(registry, package, from, to)).await;

    messages(&answer)
        .iter()
        .filter_map(|message| message["content"]["text"].as_str())
        .collect::<Vec<_>>()
        .join("\n")
}

fn review_arguments(registry: &str, package: &str, from: &str, to: &str) -> Map<String, Value> {
    let arguments = json!({
        "registry": registry,
        "package": package,
        "from_version": from,
        "to_version": to,
    });

    arguments
        .as_object()
        .expect("the arguments are an object")
        .clone()
}

async fn getting(arguments: Map<String, Value>) -> Value {
    current(json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "prompts/get",
        "params": { "name": REVIEW_UPGRADE, "arguments": arguments },
    }))
    .await
}

fn messages(answer: &Value) -> Vec<Value> {
    answer["result"]["messages"]
        .as_array()
        .unwrap_or_else(|| panic!("prompts/get should answer with messages, got {answer}"))
        .clone()
}

async fn review_upgrade() -> Value {
    let listed = prompts().await;

    listed
        .iter()
        .find(|prompt| prompt["name"] == REVIEW_UPGRADE)
        .unwrap_or_else(|| panic!("`{REVIEW_UPGRADE}` should be listed, got {listed:?}"))
        .clone()
}

async fn prompts() -> Vec<Value> {
    let answer = listing().await;

    answer["result"]["prompts"]
        .as_array()
        .unwrap_or_else(|| panic!("prompts/list should answer with an array, got {answer}"))
        .clone()
}

async fn listing() -> Value {
    current(json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "prompts/list",
        "params": {},
    }))
    .await
}

fn arguments(prompt: &Value) -> Vec<Value> {
    prompt["arguments"]
        .as_array()
        .unwrap_or_else(|| panic!("a prompt that takes arguments lists them, got {prompt}"))
        .clone()
}

fn argument_names(prompt: &Value) -> Vec<String> {
    arguments(prompt)
        .iter()
        .map(|argument| named(argument).to_owned())
        .collect()
}

fn described(prompt: &Value, field: &str) -> String {
    arguments(prompt)
        .iter()
        .find(|argument| argument["name"] == field)
        .and_then(|argument| argument["description"].as_str())
        .unwrap_or_else(|| panic!("`{field}` should be described, got {prompt}"))
        .to_owned()
}

fn named(value: &Value) -> &str {
    value["name"]
        .as_str()
        .unwrap_or_else(|| panic!("everything listed has a name, got {value}"))
}

async fn tool_surface() -> BTreeSet<String> {
    let mut surface = BTreeSet::new();

    for tool in Client::fixture().tools().await {
        surface.insert(named(&tool).to_owned());
        fields_of(&tool["inputSchema"], &mut surface);
        fields_of(&tool["outputSchema"], &mut surface);
    }

    surface
}

fn fields_of(schema: &Value, into: &mut BTreeSet<String>) {
    match schema {
        Value::Object(object) => {
            if let Some(Value::Object(properties)) = object.get("properties") {
                into.extend(properties.keys().cloned());
            }
            for nested in object.values() {
                fields_of(nested, into);
            }
        }
        Value::Array(items) => {
            for nested in items {
                fields_of(nested, into);
            }
        }
        _ => {}
    }
}

fn field_shaped_names_in(text: &str) -> BTreeSet<String> {
    text.split('`')
        .skip(1)
        .step_by(2)
        .filter(|quoted| is_field_shaped(quoted))
        .map(str::to_owned)
        .collect()
}

fn is_field_shaped(word: &str) -> bool {
    let is_identifier = word.starts_with(|c: char| c.is_ascii_lowercase())
        && word.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
    let is_compound = word.contains('_') || word.contains(|c: char| c.is_ascii_uppercase());

    is_identifier && is_compound
}

async fn current(body: Value) -> Value {
    Client::fixture().post(body).await
}

async fn previous(body: Value) -> Value {
    Client::fixture().speaking(PREVIOUS).post(body).await
}
