use diffpack_server::error::Failure;
use diffpack_server::registry::Registry;
use diffpack_server::search::{self, Search};

#[tokio::test]
async fn an_answer_this_server_cannot_read_is_not_a_query_that_matched_nothing() {
    let failure = fixtures()
        .hits(Registry::Npm, "unreadable", 200)
        .await
        .expect_err("the fixture answers in a shape this server does not know");

    match failure {
        Failure::UnreadableSearch { registry, reason } => {
            assert_eq!(registry, "npm");
            assert!(
                reason.contains("shape"),
                "the refusal should say what it objected to, got {reason}"
            );
        }
        other => panic!("an answer that will not read is not an empty page, got {other:?}"),
    }
}

#[test]
fn the_size_cap_is_above_the_index_pypi_publishes() {
    let the_index_today: u64 = 44_000_000;

    assert!(
        search::SIZE_LIMIT > the_index_today,
        "PyPI serves 44 MB and grows, got a cap of {} bytes",
        search::SIZE_LIMIT
    );
}

fn fixtures() -> Search {
    Search::fixture(concat!(env!("CARGO_MANIFEST_DIR"), "/fixtures/searches"))
}
