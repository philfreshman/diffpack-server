use diffpack_server::catalogue::{self, Catalogue};
use diffpack_server::error::Failure;
use diffpack_server::registry::Registry;

#[tokio::test]
async fn a_document_this_server_cannot_read_names_the_package_and_not_a_version() {
    let failure = fixtures()
        .versions(Registry::Npm, "unreadable")
        .await
        .expect_err("the fixture names no versions for this package");

    match failure {
        Failure::UnreadableVersions {
            registry,
            package,
            reason,
        } => {
            assert_eq!(registry, "npm");
            assert_eq!(package, "unreadable");
            assert!(
                reason.contains("shape"),
                "the refusal should say what it objected to, got {reason}"
            );
        }
        other => panic!("a document that will not read is not a missing package, got {other:?}"),
    }
}

#[test]
fn the_size_cap_is_above_what_a_large_version_document_weighs() {
    let a_large_document: u64 = 16_000_000;

    assert!(
        catalogue::SIZE_LIMIT > a_large_document,
        "npm serves 15.7 MB for `typescript`, got a cap of {} bytes",
        catalogue::SIZE_LIMIT
    );
}

fn fixtures() -> Catalogue {
    Catalogue::fixture(concat!(env!("CARGO_MANIFEST_DIR"), "/fixtures/versions"))
}
