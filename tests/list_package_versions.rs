//! `list_package_versions`, driven the way an agent drives it.
//!
//! The same two seams as `tests/list_package_files.rs`: most of what is here
//! goes over the wire, because a test that only called the handler would keep
//! passing while the definition beside it stopped matching. The handler is
//! reached directly only where the question is about the answer's *type*.
//!
//! The documents below are `fixtures/versions/`, keyed by the URL
//! `src/registry.rs` builds, so a fetch that built a URL of its own finds
//! nothing.
//!
//! # What is deliberately not asserted here
//!
//! That this answer carries `ttlMs` and `cacheScope`. #18 asks for both, and
//! a `tools/call` result has nowhere to put them: in the `2026-07-28` schema
//! `CacheableResult` is extended by `DiscoverResult`, the four list results
//! and `ReadResourceResult`, and `CallToolResult` extends plain `Result`. The
//! freshness hint this tool wants is a resource's to carry, which is #16;
//! what `tools/list` carries is `src/mcp.rs`'s and `tests/mcp.rs` holds it.
//!
//! That nothing here reaches the blob store. There is no store yet (#20,
//! #21), and when there is one `scripts/check-tool-seams.sh` is what keeps a
//! tool module from naming it — a rule held over every tool rather than a
//! fact about this one. `src/catalogue/` has no store in it at all, which is
//! the part that matters: registry metadata goes stale on its own and the
//! 256 MB budget belongs to diff results.
//!
//! That a walk of a sequence is stable, that a page stays under the response
//! ceiling, and that a cursor of a client's own invention is refused. Those
//! are `src/page.rs`'s and `tests/page.rs` holds them against a generated
//! sequence. What this suite asserts is that this tool goes *through* that
//! module rather than around it.

mod common;

use common::{Client, FIXTURES};
use diffpack_server::error::Failure;
use diffpack_server::page;
use diffpack_server::registry::Registry;
use diffpack_server::tools::list_package_versions::{Args, ListPackageVersions, Version};
use diffpack_server::tools::Ctx;
use diffpack_server::tools::Tool;
use serde_json::{json, Value};

const TOOL: &str = "list_package_versions";

// ---------------------------------------------------------------------------
// What it answers
// ---------------------------------------------------------------------------

/// The tracer bullet: one npm package, newest release first.
///
/// `zod`'s fixture is built so that the right answer is not reachable by
/// accident. Its newest release is `1.0.2` — a patch to the 1.x line
/// published after 2.0.0 was, which is what npm's own `@types/node` does
/// every week. So the expected order below is not the semver order, not the
/// lexical order of the keys, and not the order the document is written in.
/// A listing that took any of those three would have to disagree with it.
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

/// crates.io answers with an array where npm answers with an object, and its
/// dates are spelled to the microsecond where npm's are to the millisecond.
/// Out the other side they are the same listing.
///
/// The five releases below are `tokio`'s real ones, dates included, and they
/// are here because of what they are not: 1.51.4 was published between 1.52.4
/// and 1.52.3, so the order by date is not the order by version number. A
/// listing that sorted on the number would put 1.52.3 above 1.51.4.
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

/// PyPI, through deps.dev, and the case that says why the order is computed
/// from a date rather than read off the document.
///
/// deps.dev sorts a package's versions **lexically by version string**, which
/// is neither newest-first nor oldest-first. The five releases below are
/// `requests`' real ones in deps.dev's real order, and `2.9.2` is last
/// because `"2.9.2"` sorts after `"2.34.2"`. So a listing that reversed the
/// document — which is what this tool's issue originally specified — would
/// announce a 2016 release as the newest version of `requests`.
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

/// deps.dev leaves `publishedAt` off some versions — one of `requests`' 161
/// and thirty-seven of `numpy`'s 171, including ordinary-looking releases
/// like 1.10.0. They are published versions, so dropping them would answer
/// "what versions are there" with a list missing a fifth of them.
///
/// They cannot be placed in an order built from dates, so they go last: the
/// promise is newest first, and a release this server cannot date is not one
/// it can call the newest.
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

/// A scoped npm name is one package name, so it is one escaped path segment.
/// The fixture set is keyed by the URL, so a listing that interpolated
/// `@types/node` into a path — and so asked npm for a package called `node`
/// inside a scope — finds nothing here.
///
/// The five releases are `@types/node`'s real ones and are what makes the
/// case for ordering by date rather than by version number: npm's own listing
/// shows 24.13.6 on top, published forty seconds after 22.20.4 and three days
/// after 24.13.5, while `dist-tags.latest` is 26.6.2. Three different
/// questions, and this tool answers the one an agent asked.
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

/// The order is by publish date, so the date is in the answer.
///
/// Without it an agent is given a sequence it cannot check and cannot
/// explain: an undated release sits last, which looks exactly like being the
/// oldest, and a backport sitting above a newer major looks like a mistake.
/// With it, both are self-evident.
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

// ---------------------------------------------------------------------------
// Which of them are previews
// ---------------------------------------------------------------------------

/// An agent asked for "the last two versions" should not silently diff
/// against a release candidate, so every entry says whether it is one.
///
/// npm and crates.io spell a version the way semver does, so what follows the
/// first `-` is the prerelease. Build metadata is not: `2.0.1+build.5` is the
/// same release as `2.0.1` with a label on it, and flagging it would tell an
/// agent to avoid the newest stable release there is.
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

/// PyPI is not semver, and the difference is not cosmetic: PEP 440 glues its
/// markers straight onto the release, so `1.0rc1` is a release candidate and
/// there is no `-` anywhere in it. Reading it with npm's rule flags nothing.
///
/// `1.0.post1` is the other half. A post-release is a re-release of `1.0` —
/// a fixed description, a corrected classifier — not a preview of anything,
/// and flagging it would point an agent away from the newest thing there is.
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

// ---------------------------------------------------------------------------
// Which one the registry itself points at
// ---------------------------------------------------------------------------

/// The case #61 is about.
///
/// An agent asked for "the latest version of `@types/node`" and handed the
/// first entry of this listing is given 24.13.6 — a true answer to a question
/// nobody asked. It is a 24.x patch published forty seconds after the 26.6.2
/// release, which is ordinary for this package: its maintainers publish
/// across four release lines most weeks, and `npm install @types/node` gives
/// you 26.6.2.
///
/// So the two answers are different entries, and both are here. Which is
/// which comes off npm — `dist-tags.latest` was 26.6.2 when this was
/// written — rather than off what this implementation says.
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

/// crates.io carries four pointers and on a crate mid-release-cycle they
/// disagree. `leptos`'s document says all four:
///
/// ```text
/// default=0.8.20  max=0.9.0-beta  newest=0.9.0-beta  max_stable=0.8.20
/// ```
///
/// `max_version` and `newest_version` include prereleases, so reading either
/// of them answers "the latest version of `leptos`" with a beta — which is
/// the answer that makes this field worse than not having it. The fixture
/// carries all four so that reading the wrong one produces a wrong version
/// rather than nothing, and `0.9.0-beta` is also the most recently published
/// release, so the entry below cannot be reached by reading the list either.
///
/// `max_stable_version` agrees with `default_version` here and on every crate
/// checked; which of *those* two is read is a choice no fixture can force,
/// and `src/registry.rs` is where it is argued.
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

/// PyPI, through deps.dev, where the pointer is an `isDefault` on one of the
/// versions rather than a field beside them.
///
/// `jupyterlab`'s two newest releases went out ninety minutes apart on the
/// same afternoon: 4.6.4 at 15:25 and the 4.7.0a2 alpha at 16:59. So the most
/// recently published version is the alpha and the one PyPI's own project
/// page shows is 4.6.4, and an implementation that answered this field with
/// the first entry of the list — which `requests` would not have caught,
/// since its default *is* its newest publish — says the current version of
/// `jupyterlab` is an alpha.
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

// ---------------------------------------------------------------------------
// Reading it a page at a time
// ---------------------------------------------------------------------------

/// That a walk is stable — every version once, none missed, none repeated —
/// is `src/page.rs`'s property and `tests/page.rs` proves it against a
/// generated sequence. What is left for this tool is the part only it can be
/// wrong about: that the sequence reaches that module at all, so `limit` is
/// honoured and the cursor handed back is the one that resumes the walk.
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

/// The pointer is not on every page, so it is not *on* a page.
///
/// This is the failure the shape was chosen to rule out. A flag on each entry
/// would answer `limit: 1` here with one entry marked false — truthfully, and
/// indistinguishably from a package the registry points at nothing for. An
/// agent has no way to tell those two apart and no reason to ask again.
///
/// So the field is beside the page and reads the same whichever page was
/// asked for, including a page the current release is not on.
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

/// A package the registry points at nothing for still lists its versions.
///
/// npm's `latest` is a tag like any other and a maintainer can remove it —
/// `npm dist-tag rm` — leaving a package that publishes releases and names no
/// current one. The fixture keeps a `next` tag so that this also says which
/// tag is read: an implementation that took whatever tag it found would
/// answer with the release candidate, which is a preview and not a current
/// release.
///
/// Absent is not an error. The versions are still the answer to what the
/// package has released, and refusing the call would make a missing tag into
/// a missing package.
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

/// A pointer at a version that is not in the list is the registry's answer,
/// and it is passed through.
///
/// npm leaves `dist-tags.latest` where it is when a version is unpublished,
/// so a package can name a current release its own document no longer lists.
/// The version is reported as the registry spells it rather than dropped,
/// because dropping it reports *no current release* for a package that names
/// one — the same silent answer the shape of this field exists to avoid.
///
/// An agent that passes it to another tool gets a missing-version error
/// naming the version, which is a thing it can act on. Being told nothing is
/// not.
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

// ---------------------------------------------------------------------------
// When there is nothing to list
// ---------------------------------------------------------------------------

/// A package the registry does not have is something the model can act on —
/// it misspelled a name or picked the wrong registry — so it arrives as a
/// tool error it reads rather than as a protocol error it never sees.
///
/// It is the *package* that is missing and not a version, which is the whole
/// of what makes this seam's refusal different from the archive's: there is
/// no version in the request to have got wrong, so the message does not
/// suggest checking one.
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

// ---------------------------------------------------------------------------
// What a client is told
// ---------------------------------------------------------------------------

/// The arguments and the answer this tool in particular has, which is #23's
/// question asked of one tool.
///
/// The rules every tool is held to — a description, an object input schema, a
/// declared output shape, the read-only and open-world hints, the registry
/// enum — are `tests/tools.rs`'s, over all eight at once. What is here is
/// only what is true of this one.
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

    // `tests/tools.rs` requires every tool to state this one rather than
    // fixing its value, because a search answers `false`. This tool answers
    // `true`, and the sense it means is worth saying out loud: the same
    // arguments give the same answer until somebody publishes, which is the
    // window a client caches over — not a promise that the registry has
    // stopped moving.
    assert_eq!(
        tool["annotations"]["idempotentHint"], true,
        "a version list is the same answer until the next release, got {}",
        tool["annotations"]
    );
}

/// The one thing an agent cannot work out from a list of versions it is
/// shown: that the order is by publish date rather than by version number.
/// An agent that assumes otherwise reads the first entry as "the latest
/// release" and is wrong whenever a backport was the last thing published,
/// which on `@types/node` is most weeks.
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

/// The other thing an agent cannot work out from a list of versions: that
/// there is a second answer in the same result.
///
/// An agent that reads only the list will answer "what is the latest version
/// of `@types/node`" with the first entry, because that is the only answer it
/// was shown. So the description has to say that the current release is
/// carried separately, and the output schema has to name the field that
/// carries it.
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

/// The preview flag stops answering the question it is not the answer to.
///
/// Its description said: asked for the latest version, prefer the newest
/// entry where this is false. On `@types/node` that picks 24.13.6, which is
/// not a preview, is the newest thing published, and is not the current
/// release — the advice was wrong for exactly the package this whole field
/// exists for, and it is wrong the same way for any package with more than
/// one live release line.
///
/// Now there is a field that answers it, so this one says where to look
/// rather than sending an agent back to the list.
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

/// `limit` and `cursor` carry `src/page.rs`'s numbers, not numbers this tool
/// wrote down.
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

// ---------------------------------------------------------------------------
// The handler, reached directly
// ---------------------------------------------------------------------------
//
// The second seam, and a narrow one on purpose. Everything above goes over
// the wire because that is where a definition and a handler can disagree.
// What is left for these two is the part JSON cannot show: which `Failure`
// the handler returned, and that the answer carries a `Page<Version>` of
// typed values rather than a shape that happens to serialise to the right
// JSON.

/// The handler answers in the crate's own types.
#[tokio::test]
async fn the_handler_answers_with_typed_versions() {
    let answer = ListPackageVersions::call(
        Args {
            registry: Registry::Crates,
            package: "tokio".to_owned(),
            cursor: None,
            limit: None,
        },
        &Ctx::fixture(FIXTURES),
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

/// Which failure it is, rather than which words it produced.
///
/// The test over the wire asserts that the message names the package, which
/// is what a model reads. This asserts the variant, which is what decides the
/// channel it goes out on — and it is `NoSuchPackage` rather than
/// `NoSuchVersion`, because a version list is asked for by package alone.
#[tokio::test]
async fn the_handler_returns_the_failure_that_names_the_absent_package() {
    let failure = ListPackageVersions::call(
        Args {
            registry: Registry::Npm,
            package: "not-a-real-package".to_owned(),
            cursor: None,
            limit: None,
        },
        &Ctx::fixture(FIXTURES),
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

// ---------------------------------------------------------------------------
// Reading the answer
// ---------------------------------------------------------------------------

/// Each entry's version and whether it is a preview, in the order returned.
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

/// The `version` of every entry on this page, in the order they were returned.
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

/// The listed definition of `name`, or a panic naming what was listed.
async fn listed(name: &str) -> Value {
    Client::fixture().listed(name).await
}

/// Call this tool with `arguments`, returning the `result`.
async fn call(arguments: Value) -> Value {
    Client::fixture().call(TOOL, arguments).await
}
