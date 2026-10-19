mod common;

use common::{names, Client, FIXTURES};
use diffpack_server::error::Failure;
use diffpack_server::page;
use diffpack_server::registry::{PackageName, Registry};
use diffpack_server::tools::list_package_versions::{Args, ListPackageVersions, Version};
use diffpack_server::tools::Tool;
use diffpack_server::tools::{Call, Ctx};
use serde_json::{json, Value};

const TOOL: &str = "list_package_versions";

#[tokio::test]
async fn an_npm_packages_versions_come_back_newest_first() {
    let result = call(json!({
        "registry": "npm",
        "package": "zod",
    }))
    .await;

    assert_eq!(
        result["isError"],
        json!(false),
        "a package the fixture set has is not an error, got {result}"
    );

    assert_eq!(
        versions(&result),
        vec!["1.0.2", "1.0.10", "2.0.0", "1.0.0"],
        "newest first means most recently published first: 1.0.2 is a patch \
         to the 1.x line published after 2.0.0, so a listing sorted by \
         version number or by the document's own order gets this wrong: got \
         {result}"
    );
}

#[tokio::test]
async fn a_crates_io_packages_versions_come_back_newest_first() {
    let result = call(json!({
        "registry": "crates",
        "package": "tokio",
    }))
    .await;

    assert_eq!(
        result["isError"],
        json!(false),
        "a crate the fixture set has is not an error, got {result}"
    );

    assert_eq!(
        versions(&result),
        vec!["1.53.1", "1.53.0", "1.52.4", "1.51.4", "1.52.3"],
        "a backport published between two releases of a newer line sits where \
         its date puts it, not where its version number would: got {result}"
    );
}

#[tokio::test]
async fn a_pypi_packages_versions_are_ordered_by_date_not_by_the_documents_order() {
    let result = call(json!({
        "registry": "pypi",
        "package": "requests",
    }))
    .await;

    assert_eq!(
        result["isError"],
        json!(false),
        "a package the fixture set has is not an error, got {result}"
    );

    assert_eq!(
        versions(&result),
        vec![
            "2.34.2",
            "2.31.0",
            "2.9.2",
            "2.9.0",
            "0.10.0",
            "2.23.0-py2.7"
        ],
        "deps.dev lists these lexically, so the last entry is 2.9.2 and \
         reversing the document would put a 2016 release on top: got {result}"
    );
}

#[tokio::test]
async fn a_go_modules_versions_come_back_highest_first_because_the_proxy_dates_none() {
    let result = call(json!({
        "registry": "go",
        "package": "github.com/go-chi/chi",
    }))
    .await;

    assert_eq!(
        result["isError"],
        json!(false),
        "a module the fixture set has is not an error, got {result}"
    );

    assert_eq!(
        versions(&result),
        vec![
            "v4.1.2+incompatible",
            "v4.1.1+incompatible",
            "v4.1.0+incompatible",
            "v4.0.4+incompatible",
            "v4.0.3+incompatible",
            "v4.0.2+incompatible",
            "v4.0.1+incompatible",
            "v4.0.0+incompatible",
            "v4.0.0-rc2+incompatible",
            "v3.3.4+incompatible",
            "v3.3.3+incompatible",
            "v3.3.2+incompatible",
            "v3.3.1+incompatible",
            "v3.3.0+incompatible",
            "v3.2.1+incompatible",
            "v3.2.0+incompatible",
            "v3.1.5+incompatible",
            "v3.1.4+incompatible",
            "v3.1.3+incompatible",
            "v3.1.2+incompatible",
            "v3.1.1+incompatible",
            "v3.1.0+incompatible",
            "v3.0.0+incompatible",
            "v2.1.0+incompatible",
            "v2.0.0+incompatible",
            "v1.5.5",
            "v1.5.4",
            "v1.5.3",
            "v1.5.2",
            "v1.5.1",
            "v1.5.0",
            "v1.0.0",
            "v0.9.0",
        ],
        "the proxy lists versions in no order and with no dates, so they are \
         ordered the way diffpack.io orders them: by version, a release above \
         its own previews, build metadata ignored: got {result}"
    );

    let items = result["structuredContent"]["versions"]["items"]
        .as_array()
        .unwrap_or_else(|| panic!("a page carries its items, got {result}"));
    assert!(
        items.iter().all(|item| item["publishedAt"].is_null()),
        "the proxy's list carries no dates, and none is made up: got {result}"
    );
}

#[tokio::test]
async fn a_go_modules_current_version_is_what_latest_resolves_and_not_the_top_of_the_list() {
    let result = call(json!({
        "registry": "go",
        "package": "github.com/go-chi/chi",
    }))
    .await;

    assert_eq!(
        result["structuredContent"]["currentVersion"],
        json!("v1.5.5"),
        "`go get github.com/go-chi/chi` resolves v1.5.5, because the proxy's \
         `@latest` passes over `+incompatible` releases when a compatible one \
         exists: got {result}"
    );
}

#[tokio::test]
async fn an_untagged_go_module_is_the_one_version_latest_resolves() {
    let result = call(json!({
        "registry": "go",
        "package": "golang.org/x/exp",
    }))
    .await;

    let items = result["structuredContent"]["versions"]["items"]
        .as_array()
        .unwrap_or_else(|| panic!("a page carries its items, got {result}"));

    assert_eq!(
        items,
        &vec![json!({
            "version": "v0.0.0-20261014143641-f0251eb4aa3e",
            "publishedAt": "2026-10-14T14:36:41Z",
            "prerelease": true,
        })],
        "a module that was never tagged lists nothing, and the proxy resolves a \
         pseudo-version of its default branch instead, dated and a preview by \
         semver's own rule: got {result}"
    );
    assert_eq!(
        result["structuredContent"]["currentVersion"],
        json!("v0.0.0-20261014143641-f0251eb4aa3e"),
        "and that pseudo-version is what `go get` installs: got {result}"
    );
}

#[tokio::test]
async fn a_go_list_is_answered_when_only_latest_fails() {
    let result = call(json!({
        "registry": "go",
        "package": "example.com/latest-unreachable",
    }))
    .await;

    assert_eq!(
        result["isError"],
        json!(false),
        "the list answered, and `@latest` is asked only for the pointer beside \
         it: got {result}"
    );
    assert_eq!(versions(&result), vec!["v1.1.0", "v1.0.0"], "got {result}");
    assert!(
        result["structuredContent"]["currentVersion"].is_null(),
        "with no answer from `@latest` there is no pointer to report, rather \
         than one guessed from the list: got {result}"
    );
}

#[tokio::test]
async fn a_go_prerelease_is_flagged_and_incompatible_build_metadata_is_not() {
    let result = call(json!({
        "registry": "go",
        "package": "github.com/go-chi/chi",
    }))
    .await;

    let flagged: Vec<(&str, bool)> = previews(&result)
        .into_iter()
        .filter(|(version, _)| version.starts_with("v4.0.0"))
        .collect();

    assert_eq!(
        flagged,
        vec![
            ("v4.0.0+incompatible", false),
            ("v4.0.0-rc2+incompatible", true),
        ],
        "a Go version is semver, so `+incompatible` is build metadata and `-rc2` \
         is the preview: got {result}"
    );
}

#[tokio::test]
async fn a_version_the_source_gives_no_date_for_is_listed_last_rather_than_dropped() {
    let result = call(json!({
        "registry": "pypi",
        "package": "requests",
    }))
    .await;

    let listed = versions(&result);

    assert!(
        listed.contains(&"2.23.0-py2.7"),
        "a version with no date is still a version the package has, got {result}"
    );
    assert_eq!(
        listed.last(),
        Some(&"2.23.0-py2.7"),
        "an undated release cannot be claimed to be the newest, so it sits \
         after every release that can be dated: got {result}"
    );
}

#[tokio::test]
async fn a_scoped_npm_package_is_asked_for_under_its_whole_name() {
    let result = call(json!({
        "registry": "npm",
        "package": "@types/node",
    }))
    .await;

    assert_eq!(
        result["isError"],
        json!(false),
        "a scoped package the fixture set has is not an error, got {result}"
    );

    assert_eq!(
        versions(&result),
        vec!["24.13.6", "22.20.4", "25.9.8", "26.6.2", "24.13.5"],
        "the newest release of this package is a 24.x patch and the second \
         newest is a 22.x one, which is neither the semver order nor the \
         `latest` tag: got {result}"
    );
}

#[tokio::test]
async fn an_answer_says_when_each_version_was_published() {
    let result = call(json!({
        "registry": "pypi",
        "package": "requests",
    }))
    .await;

    let items = result["structuredContent"]["versions"]["items"]
        .as_array()
        .unwrap_or_else(|| panic!("a page carries its items, got {result}"));

    assert_eq!(
        items[0]["publishedAt"], "2026-05-14T19:25:26Z",
        "the date is the registry's own, passed through rather than reformatted: got {result}"
    );

    let undated = items
        .last()
        .unwrap_or_else(|| panic!("the package has versions, got {result}"));
    assert_eq!(undated["version"], "2.23.0-py2.7", "got {result}");
    assert!(
        undated["publishedAt"].is_null(),
        "a version the source gave no date for says so rather than being \
         given one: got {undated}"
    );
}

#[tokio::test]
async fn an_npm_prerelease_is_flagged_and_build_metadata_is_not() {
    let result = call(json!({
        "registry": "npm",
        "package": "prereleases",
    }))
    .await;

    assert_eq!(
        previews(&result),
        vec![
            ("2.0.1+build.5", false),
            ("2.0.0", false),
            ("2.0.0-rc.1", true),
            ("2.0.0-alpha.1", true),
            ("1.0.0", false),
        ],
        "semver's prerelease is what follows the first `-`, and `+build.5` is \
         not one: got {result}"
    );
}

#[tokio::test]
async fn a_pypi_prerelease_is_flagged_and_a_post_release_is_not() {
    let result = call(json!({
        "registry": "pypi",
        "package": "prereleases",
    }))
    .await;

    assert_eq!(
        previews(&result),
        vec![
            ("1.0.post1", false),
            ("1.0", false),
            ("1.0rc1", true),
            ("1.0b2", true),
            ("1.0a1", true),
            ("1.0.dev1", true),
        ],
        "PEP 440 needs no separator before its marker, and a post-release is \
         not a preview: got {result}"
    );
}

#[tokio::test]
async fn an_npm_packages_current_version_is_the_tag_and_not_the_newest_publish() {
    let result = call(json!({
        "registry": "npm",
        "package": "@types/node",
    }))
    .await;

    assert_eq!(
        result["structuredContent"]["currentVersion"],
        json!("26.6.2"),
        "npm points at 26.6.2 with `dist-tags.latest`, which is what \
         `npm install` resolves: got {result}"
    );

    assert_eq!(
        versions(&result).first(),
        Some(&"24.13.6"),
        "the most recently published version is a 24.x patch. It is the other \
         question this tool answers, and answering this one with it is the \
         whole reason the field above exists: got {result}"
    );
}

#[tokio::test]
async fn a_crates_io_crates_current_version_is_stable_when_the_newest_release_is_a_preview() {
    let result = call(json!({
        "registry": "crates",
        "package": "leptos",
    }))
    .await;

    assert_eq!(
        result["structuredContent"]["currentVersion"],
        json!("0.8.20"),
        "`cargo add leptos` resolves 0.8.20, which is what crates.io's own \
         page defaults to: got {result}"
    );

    assert_eq!(
        versions(&result).first(),
        Some(&"0.9.0-beta"),
        "the most recently published release is the preview, which is the \
         answer the other two pointers would have given: got {result}"
    );
}

#[tokio::test]
async fn a_pypi_packages_current_version_is_the_default_and_not_the_newest_publish() {
    let result = call(json!({
        "registry": "pypi",
        "package": "jupyterlab",
    }))
    .await;

    assert_eq!(
        result["structuredContent"]["currentVersion"],
        json!("4.6.4"),
        "deps.dev marks 4.6.4 `isDefault`, which is what `pip install \
         jupyterlab` resolves: got {result}"
    );

    assert_eq!(
        versions(&result).first(),
        Some(&"4.7.0a2"),
        "the alpha went out ninety minutes after 4.6.4, so it is the newest \
         publish and not the current release: got {result}"
    );
}

#[tokio::test]
async fn a_walk_of_the_pages_is_the_whole_listing() {
    let whole: Vec<String> =
        versions(&call(json!({ "registry": "pypi", "package": "requests" })).await)
            .iter()
            .map(|version| version.to_string())
            .collect();

    let first = call(json!({
        "registry": "pypi", "package": "requests", "limit": 2,
    }))
    .await;

    assert_eq!(
        versions(&first),
        &whole[..2],
        "a page of two is the first two, got {first}"
    );
    assert_eq!(
        first["structuredContent"]["versions"]["total"],
        json!(6),
        "the total is how many versions the package has and not how many are \
         on this page — an agent told it received 2 of 2 has no reason to ask \
         again: got {first}"
    );

    let cursor = first["structuredContent"]["versions"]["nextCursor"]
        .as_str()
        .unwrap_or_else(|| panic!("a page that ends early hands back a cursor, got {first}"))
        .to_owned();

    let rest = call(json!({
        "registry": "pypi", "package": "requests", "cursor": cursor,
    }))
    .await;

    assert_eq!(
        versions(&rest),
        &whole[2..],
        "the cursor resumes where the page stopped, got {rest}"
    );
    assert!(
        rest["structuredContent"]["versions"]["nextCursor"].is_null(),
        "the last page of a sequence does not hand out another cursor, got {rest}"
    );
}

#[tokio::test]
async fn the_current_version_is_the_same_answer_on_a_page_it_is_not_on() {
    let page = call(json!({
        "registry": "npm",
        "package": "@types/node",
        "limit": 1,
    }))
    .await;

    assert_eq!(
        versions(&page),
        vec!["24.13.6"],
        "a page of one is the newest publish alone, got {page}"
    );
    assert_eq!(
        page["structuredContent"]["versions"]["total"],
        json!(5),
        "the total is the package's, so an agent can see there is more, got {page}"
    );

    assert_eq!(
        page["structuredContent"]["currentVersion"],
        json!("26.6.2"),
        "26.6.2 is not on this page, and a page it is not on is not a package \
         without a current release: got {page}"
    );
}

#[tokio::test]
async fn a_package_the_registry_points_at_nothing_for_is_not_an_error() {
    let result = call(json!({
        "registry": "npm",
        "package": "untagged",
    }))
    .await;

    assert_eq!(
        result["isError"],
        json!(false),
        "a package with no `latest` tag is still a package, got {result}"
    );
    assert_eq!(
        versions(&result),
        vec!["3.0.0-rc.1", "2.0.0"],
        "the listing is unaffected by the tag being absent, got {result}"
    );

    assert!(
        result["structuredContent"]["currentVersion"].is_null(),
        "no current release is said rather than guessed at, and the `next` \
         tag is not it: got {result}"
    );
}

#[tokio::test]
async fn a_pointer_at_a_version_that_is_not_listed_is_reported_rather_than_dropped() {
    let result = call(json!({
        "registry": "npm",
        "package": "unpublished-latest",
    }))
    .await;

    assert_eq!(
        result["isError"],
        json!(false),
        "a tag pointing past the list is not a package this server refuses, got {result}"
    );
    assert_eq!(
        versions(&result),
        vec!["1.0.1", "1.0.0"],
        "the listing is what the document lists, got {result}"
    );

    assert_eq!(
        result["structuredContent"]["currentVersion"],
        json!("2.0.0"),
        "npm says 2.0.0 is current and this answer says what npm says, got {result}"
    );
}

#[tokio::test]
async fn a_package_the_registry_does_not_have_is_a_tool_error_naming_it() {
    let result = call(json!({
        "registry": "npm",
        "package": "not-a-real-package",
    }))
    .await;

    assert_eq!(
        result["isError"],
        json!(true),
        "a package that is not there is the model's to act on, got {result}"
    );

    let message = result["content"][0]["text"]
        .as_str()
        .unwrap_or_else(|| panic!("a tool error carries text a model reads, got {result}"));

    assert!(
        message.contains("not-a-real-package") && message.contains("npm"),
        "the message names what was asked for and where, got {message}"
    );
}

#[tokio::test]
async fn a_go_module_the_proxy_does_not_have_is_a_tool_error_naming_it() {
    let result = call(json!({
        "registry": "go",
        "package": "github.com/not-a-real/module",
    }))
    .await;

    assert_eq!(
        result["isError"],
        json!(true),
        "a module that is not there is the model's to act on, got {result}"
    );

    let message = result["content"][0]["text"]
        .as_str()
        .unwrap_or_else(|| panic!("a tool error carries text a model reads, got {result}"));

    assert!(
        message.contains("github.com/not-a-real/module") && names(message, "Go"),
        "the message names what was asked for and where, got {message}"
    );
}

#[tokio::test]
async fn the_definition_carries_everything_an_agent_needs() {
    let tool = listed(TOOL).await;

    for field in ["registry", "package", "cursor", "limit"] {
        assert!(
            tool["inputSchema"]["properties"][field].is_object(),
            "the input schema should describe `{field}`, got {}",
            tool["inputSchema"]
        );
    }
    for required in ["registry", "package"] {
        assert!(
            tool["inputSchema"]["required"]
                .as_array()
                .is_some_and(|fields| fields.iter().any(|field| field == required)),
            "`{required}` is not optional, got {}",
            tool["inputSchema"]
        );
    }
    assert!(
        !tool["inputSchema"]["properties"]["version"].is_object(),
        "this tool answers what the versions *are*, so asking for one would \
         be asking the question backwards: got {}",
        tool["inputSchema"]
    );

    assert_eq!(
        tool["annotations"]["idempotentHint"], true,
        "a version list is the same answer until the next release, got {}",
        tool["annotations"]
    );
}

#[tokio::test]
async fn the_description_says_the_order_is_by_date_rather_than_by_number() {
    let tool = listed(TOOL).await;
    let description = tool["description"].as_str().unwrap_or_else(|| {
        panic!("a tool an agent picks without documentation has one, got {tool}")
    });

    assert!(
        description.contains("publish date") && description.contains("version number"),
        "the description should say which of the two orders this is, since \
         an agent cannot tell from the answer: got {description}"
    );
}

#[tokio::test]
async fn the_description_says_a_go_modules_order_is_by_version_number() {
    let tool = listed(TOOL).await;
    let description = tool["description"].as_str().unwrap_or_else(|| {
        panic!("a tool an agent picks without documentation has one, got {tool}")
    });

    assert!(
        names(description, "Go"),
        "the one registry whose list is not by publish date should be named \
         where the order is promised, got {description}"
    );

    let published_at = tool["outputSchema"]["$defs"]["Version"]["properties"]["publishedAt"]
        ["description"]
        .as_str()
        .unwrap_or_else(|| panic!("every field is described, got {}", tool["outputSchema"]));
    assert!(
        names(published_at, "Go"),
        "the field whose absence changes the order says when it is absent, \
         got {published_at}"
    );
}

#[tokio::test]
async fn the_definition_says_the_current_release_is_answered_separately() {
    let tool = listed(TOOL).await;

    let current = &tool["outputSchema"]["properties"]["currentVersion"];
    assert!(
        current.is_object(),
        "the answer's second field is declared where a client validates \
         against it, got {}",
        tool["outputSchema"]
    );
    assert!(
        current["description"]
            .as_str()
            .is_some_and(|said| said.contains("not")),
        "the description has to say what this is *not* — the first entry of \
         the list — since that is the answer an agent would otherwise take: \
         got {current}"
    );

    let description = tool["description"].as_str().unwrap_or_else(|| {
        panic!("a tool an agent picks without documentation has one, got {tool}")
    });
    assert!(
        description.contains("currentVersion"),
        "an agent choosing this tool reads the description, and a field it \
         is never told about is one it never looks at: got {description}"
    );
}

#[tokio::test]
async fn the_preview_flag_sends_an_agent_to_the_current_version_rather_than_the_list() {
    let tool = listed(TOOL).await;

    let said = tool["outputSchema"]["$defs"]["Version"]["properties"]["prerelease"]["description"]
        .as_str()
        .unwrap_or_else(|| {
            panic!(
                "every field of an answer is described where a client reads it, got {}",
                tool["outputSchema"]
            )
        });

    assert!(
        said.contains("currentVersion"),
        "a preview flag that tells an agent to pick the newest entry that is \
         not one is telling it to answer the current release with a backport: \
         got {said}"
    );
}

#[tokio::test]
async fn the_paging_arguments_document_the_numbers_that_bind() {
    let tool = listed(TOOL).await;
    let limit = &tool["inputSchema"]["properties"]["limit"];

    assert_eq!(limit["default"], json!(page::DEFAULT_LIMIT), "got {limit}");
    assert_eq!(limit["maximum"], json!(page::MAX_LIMIT), "got {limit}");
    assert!(
        limit["description"]
            .as_str()
            .is_some_and(|said| said.contains("clamped")),
        "the description is the one `page::Limit` writes, got {limit}"
    );

    let cursor = &tool["inputSchema"]["properties"]["cursor"];
    assert_eq!(cursor["pattern"], "^p1:[0-9]+$", "got {cursor}");
}

#[tokio::test]
async fn the_handler_answers_with_typed_versions() {
    let answer = ListPackageVersions::call(
        Args {
            registry: Registry::Crates,
            package: PackageName::new("tokio"),
            cursor: None,
            limit: None,
        },
        &Call::new(&Ctx::fixture(FIXTURES)),
    )
    .await
    .expect("the fixture set has this crate");

    assert_eq!(
        answer.versions.items.first(),
        Some(&Version {
            version: "1.53.1".to_owned(),
            published_at: Some("2026-07-20T17:06:09.996426Z".to_owned()),
            prerelease: false,
        }),
        "a `prerelease` that serialised correctly by accident — a string, a \
         number — would pass every test above and fail a client validating \
         against the schema"
    );
    assert_eq!(answer.versions.total, 5);
    assert_eq!(answer.versions.next_cursor, None);
}

#[tokio::test]
async fn the_handler_returns_the_failure_that_names_the_absent_package() {
    let failure = ListPackageVersions::call(
        Args {
            registry: Registry::Npm,
            package: PackageName::new("not-a-real-package"),
            cursor: None,
            limit: None,
        },
        &Call::new(&Ctx::fixture(FIXTURES)),
    )
    .await
    .expect_err("the fixture set says this URL serves nothing");

    match failure {
        Failure::NoSuchPackage { registry, package } => {
            assert_eq!(package, "not-a-real-package");
            assert_eq!(registry, "npm");
        }
        other => panic!("a missing package should say so, got {other:?}"),
    }
}

fn previews(result: &Value) -> Vec<(&str, bool)> {
    result["structuredContent"]["versions"]["items"]
        .as_array()
        .unwrap_or_else(|| panic!("a page carries its items, got {result}"))
        .iter()
        .map(|entry| {
            let version = entry["version"]
                .as_str()
                .unwrap_or_else(|| panic!("every entry names a version, got {entry}"));
            let prerelease = entry["prerelease"].as_bool().unwrap_or_else(|| {
                panic!("every entry says whether it is a prerelease, got {entry}")
            });
            (version, prerelease)
        })
        .collect()
}

fn versions(result: &Value) -> Vec<&str> {
    result["structuredContent"]["versions"]["items"]
        .as_array()
        .unwrap_or_else(|| panic!("a page carries its items, got {result}"))
        .iter()
        .map(|entry| {
            entry["version"]
                .as_str()
                .unwrap_or_else(|| panic!("every entry names a version, got {entry}"))
        })
        .collect()
}

async fn listed(name: &str) -> Value {
    Client::fixture().listed(name).await
}

async fn call(arguments: Value) -> Value {
    Client::fixture().call(TOOL, arguments).await
}
