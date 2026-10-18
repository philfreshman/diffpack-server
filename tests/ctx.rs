use std::path::Path;
use std::time::Duration;

mod common;

use common::{Client, FIXTURES};
use diffpack_server::tools::Ctx;
use serde_json::{json, Value};

const NO_FIXTURES: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/fixtures/no-set-is-checked-in-here"
);

struct Seam {
    name: &'static str,

    tool: &'static str,
    arguments: Value,

    calls: usize,

    reads: &'static str,
    fixture_says: Value,

    doing: Option<&'static str>,
}

fn seams() -> Vec<Seam> {
    vec![
        Seam {
            name: "archive",
            tool: "get_file_content",
            arguments: json!({
                "registry": "crates",
                "package": "serde",
                "version": "1.0.0",
                "path": "src/lib.rs",
            }),
            calls: 1,
            reads: "/text",
            fixture_says: json!("pub fn serialize() {}\n"),
            doing: Some("reading the archive fixtures"),
        },
        Seam {
            name: "catalogue",
            tool: "list_package_versions",
            arguments: json!({ "registry": "npm", "package": "zod" }),
            calls: 1,
            reads: "/versions/total",
            fixture_says: json!(4),
            doing: Some("reading the version fixtures"),
        },
        Seam {
            name: "search",
            tool: "search_packages",
            arguments: json!({ "registry": "npm", "query": "zod" }),
            calls: 1,
            reads: "/total",
            fixture_says: json!(2),
            doing: Some("reading the search fixtures"),
        },
        Seam {
            name: "store",
            tool: "diff_package_versions",
            arguments: json!({
                "registry": "npm",
                "package": "diffable",
                "from_version": "1.0.0",
                "to_version": "2.0.0",
            }),
            calls: 2,
            reads: "/cached",
            fixture_says: json!(true),
            doing: None,
        },
    ]
}

#[test]
fn every_seam_a_context_carries_is_driven_here() {
    let mut driven: Vec<&str> = seams().iter().map(|seam| seam.name).collect();
    driven.sort_unstable();

    let mut carried = Ctx::fixture(FIXTURES).seams().to_vec();
    carried.sort_unstable();

    assert_eq!(
        driven, carried,
        "a seam with no call here is a seam nothing holds to the fixture \
         set: give it the shortest call that reaches it"
    );
}

#[tokio::test]
async fn every_seam_a_context_carries_is_the_fixture_set() {
    for seam in seams() {
        let answer = call(FIXTURES, &seam).await;
        let read = answer["result"]["structuredContent"].pointer(seam.reads);

        assert_eq!(
            read,
            Some(&seam.fixture_says),
            "the `{}` seam should have read the fixture set, got {answer}",
            seam.name
        );
    }
}

#[tokio::test]
async fn no_seam_a_context_carries_can_reach_a_registry() {
    assert!(
        !Path::new(NO_FIXTURES).exists(),
        "`{NO_FIXTURES}` has to hold nothing for this test to mean anything, \
         and something is checked in there"
    );

    for seam in seams() {
        let Some(doing) = seam.doing else {
            continue;
        };

        let answer = call(NO_FIXTURES, &seam).await;

        assert_eq!(
            answer["error"]["code"], -32000,
            "the `{}` seam should have failed reading a fixture set that is \
             not there, got {answer}",
            seam.name
        );
        assert_eq!(
            answer["error"]["message"],
            format!("diffpack failed while {doing}."),
            "the `{}` seam should have gone to the fixture adapter, got \
             {answer}",
            seam.name
        );
    }
}

async fn call(fixtures: &'static str, seam: &Seam) -> Value {
    let client = Client::over(Ctx::fixture(fixtures));

    let mut answer = Value::Null;
    for request in 0..seam.calls {
        answer = client
            .post(json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "tools/call",
                "params": { "name": seam.tool, "arguments": seam.arguments },
            }))
            .await;

        if request + 1 < seam.calls {
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }
    answer
}
