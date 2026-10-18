use diffpack_server::archive::Archive;
use diffpack_server::catalogue::Catalogue;
use diffpack_server::error::Failure;
use diffpack_server::registry::Registry;
use diffpack_server::search::Search;

#[tokio::test]
async fn a_body_over_the_cap_is_refused_in_the_words_of_the_seam_that_asked() {
    let archive = Archive::fixture(set("archives"))
        .with_limit(64)
        .fetch(Registry::Npm, "@types/node", "20.1.0")
        .await
        .expect_err("64 bytes is smaller than any real archive");

    match archive {
        Failure::TooLarge {
            package,
            version,
            bytes,
            limit,
        } => {
            assert_eq!(package, "@types/node");
            assert_eq!(version, "20.1.0");
            assert_eq!(limit, 64, "the refusal names the limit that was applied");
            assert!(
                bytes > 64,
                "the refusal names what was on offer, got {bytes} bytes"
            );
        }
        other => panic!("an archive over the cap is `too_large`, got {other:?}"),
    }

    let versions = Catalogue::fixture(set("versions"))
        .with_limit(64)
        .versions(Registry::Crates, "tokio")
        .await
        .expect_err("64 bytes is smaller than any real document");

    match versions {
        Failure::VersionsTooLarge {
            registry,
            package,
            bytes,
            limit,
        } => {
            assert_eq!(registry, "crates.io");
            assert_eq!(package, "tokio");
            assert_eq!(limit, 64, "the refusal names the limit that was applied");
            assert!(
                bytes > 64,
                "the refusal names what was on offer, got {bytes} bytes"
            );
        }
        other => panic!("a version list over the cap is `versions_too_large`, got {other:?}"),
    }

    let hits = Search::fixture(set("searches"))
        .with_limit(64)
        .hits(Registry::Crates, "serde", 200)
        .await
        .expect_err("64 bytes is smaller than any real answer");

    match hits {
        Failure::SearchTooLarge {
            registry,
            bytes,
            limit,
        } => {
            assert_eq!(registry, "crates.io");
            assert_eq!(limit, 64, "the refusal names the limit that was applied");
            assert!(
                bytes > 64,
                "the refusal names what was on offer, got {bytes} bytes"
            );
        }
        other => panic!("a search answer over the cap is `search_too_large`, got {other:?}"),
    }
}

fn set(name: &str) -> String {
    format!("{}/fixtures/{name}", env!("CARGO_MANIFEST_DIR"))
}
