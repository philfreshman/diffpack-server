use std::sync::{Arc, OnceLock, RwLock};
use std::time::{Duration, Instant};

use tokio::sync::{Mutex, MutexGuard};

use crate::error::Failure;
use crate::fetch::{self, About as Fetching};

use super::{weighed, About, Body};

#[derive(Debug)]
pub struct Live;

impl Live {
    pub fn new() -> Self {
        Self
    }

    pub async fn body(&self, url: &str, limit: u64, about: &About<'_>) -> Result<Body, Failure> {
        let Some(fresh_for) = about.remember_for else {
            return download(url, limit, about).await.map(Body::Owned);
        };

        match remembered(url, fresh_for) {
            Some(body) => weighed(Body::Shared(body), limit, about),
            None => fetched_once(url, limit, fresh_for, about).await,
        }
    }
}

static TURN_TO_FETCH: Mutex<()> = Mutex::const_new(());

const ONE_TURN: Duration = fetch::LONGEST_FETCH;

async fn fetched_once(
    url: &str,
    limit: u64,
    fresh_for: Duration,
    about: &About<'_>,
) -> Result<Body, Failure> {
    let _turn = turn_to_fetch().await?;

    if let Some(body) = remembered(url, fresh_for) {
        return weighed(Body::Shared(body), limit, about);
    }

    let body: Arc<[u8]> = Arc::from(download(url, limit, about).await?);
    remember(url, &body);
    Ok(Body::Shared(body))
}

async fn turn_to_fetch() -> Result<MutexGuard<'static, ()>, Failure> {
    tokio::time::timeout(ONE_TURN, TURN_TO_FETCH.lock())
        .await
        .map_err(|_| Failure::Busy { waited: ONE_TURN })
}

async fn download(url: &str, limit: u64, about: &About<'_>) -> Result<Vec<u8>, Failure> {
    fetch::bytes(
        url,
        limit,
        &Fetching {
            registry: about.registry,
            accept: about.accept,
            compressed: about.compressed,
            missing: about.missing,
            too_large: about.too_large,
        },
    )
    .await
}

fn remembered(url: &str, fresh_for: Duration) -> Option<Arc<[u8]>> {
    let memo = memo().read().ok()?;
    let held = memo.as_ref()?;

    (held.url == url && held.fetched.elapsed() < fresh_for).then(|| Arc::clone(&held.body))
}

fn remember(url: &str, body: &Arc<[u8]>) {
    if let Ok(mut memo) = memo().write() {
        *memo = Some(Held {
            url: url.to_owned(),
            fetched: Instant::now(),
            body: Arc::clone(body),
        });
    }
}

fn memo() -> &'static RwLock<Option<Held>> {
    static MEMO: OnceLock<RwLock<Option<Held>>> = OnceLock::new();
    MEMO.get_or_init(|| RwLock::new(None))
}

struct Held {
    url: String,
    fetched: Instant,
    body: Arc<[u8]>,
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::collections::BTreeSet;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use axum::extract::State;
    use axum::http::StatusCode;
    use axum::routing::get;
    use axum::Router;
    use tokio::sync::watch;

    use crate::fetch::tests::ONE_AT_A_TIME;
    use crate::fetch::{DOWNLOADS_AT_ONCE, LONGEST_FETCH};
    use crate::registry::Registry;

    const BURST: usize = DOWNLOADS_AT_ONCE * 2;

    const LIMIT: u64 = 1024;

    const INDEX: &str = "every package there is";

    const ARCHIVE: &str = "one version of one package";

    #[tokio::test]
    async fn a_cold_burst_fetches_the_index_once() {
        let _alone = ONE_AT_A_TIME.lock().await;
        let (urls, stub) = serve(Index::Served).await;

        burst(&urls.index).await;

        assert_eq!(
            stub.asked(),
            1,
            "{BURST} searches arriving together on a cold instance should cost one fetch of \
             the index"
        );
    }

    #[tokio::test]
    async fn everyone_in_a_cold_burst_is_answered_with_the_index_as_served() {
        let _alone = ONE_AT_A_TIME.lock().await;
        let (urls, _) = serve(Index::Served).await;

        for answer in burst(&urls.index).await {
            let body = answer.expect("the stub serves the index to whoever asks");
            assert_eq!(
                &*body,
                INDEX.as_bytes(),
                "a search that waited should be answered with what a search alone would get"
            );
        }
    }

    #[tokio::test]
    async fn a_cold_burst_leaves_a_download_slot_for_an_archive() {
        let _alone = ONE_AT_A_TIME.lock().await;
        let (urls, stub) = serve(Index::HeldUntilAnArchiveArrives).await;
        let live = Live::new();
        let a_document = a_document();

        let (searches, archive) = futures::join!(
            burst(&urls.index),
            live.body(&urls.archive, LIMIT, &a_document),
        );

        assert!(
            archive.is_ok(),
            "an archive asked for during a burst of index fetches should get a download slot, \
             got {archive:?}"
        );
        assert!(
            searches.iter().all(Result::is_ok),
            "the index is served once the archive has arrived, got {searches:?}"
        );
        assert_eq!(
            stub.most_at_once(),
            1,
            "a burst of index fetches should hold one download slot at a time"
        );
    }

    #[tokio::test]
    async fn a_failed_fetch_is_not_remembered() {
        let _alone = ONE_AT_A_TIME.lock().await;
        let (urls, stub) = serve(Index::RefusedWithItsOwnStatus).await;

        let statuses: BTreeSet<u16> = burst(&urls.index)
            .await
            .into_iter()
            .map(|answer| match answer {
                Err(Failure::Unavailable { status, .. }) => status,
                other => panic!("the stub refuses every request for the index, got {other:?}"),
            })
            .collect();

        assert_eq!(
            stub.asked(),
            BURST,
            "every caller after a failed fetch should fetch again rather than inherit the failure"
        );
        assert_eq!(
            statuses.len(),
            BURST,
            "each caller should get back the failure of its own fetch, got {statuses:?}"
        );
    }

    #[tokio::test]
    async fn a_document_nobody_remembers_does_not_wait_for_the_index() {
        let _alone = ONE_AT_A_TIME.lock().await;
        let (urls, _) = serve(Index::HeldUntilAnArchiveArrives).await;
        let live = Live::new();
        let (the_index, a_document) = (the_index(), a_document());

        let (index, archive) = futures::join!(
            live.body(&urls.index, LIMIT, &the_index),
            live.body(&urls.archive, LIMIT, &a_document),
        );

        assert!(
            index.is_ok() && archive.is_ok(),
            "the index is served only once the archive has arrived, so an archive waiting on \
             the index's turn leaves both waiting, got {index:?} and {archive:?}"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn a_caller_that_waits_out_a_full_turn_is_refused_as_busy() {
        let _alone = ONE_AT_A_TIME.lock().await;
        let (urls, stub) = serve(Index::Served).await;

        let refused = refused_for_want_of_a_turn(&urls.index).await;

        assert_eq!(
            refused.kind(),
            "busy",
            "the wait was for this server's own turn and not a registry's answer, got {refused:?}"
        );
        assert_eq!(
            stub.asked(),
            0,
            "a caller refused for want of a turn should never reach the registry"
        );
    }

    async fn refused_for_want_of_a_turn(url: &str) -> Failure {
        let _held = TURN_TO_FETCH.lock().await;

        tokio::time::timeout(
            LONGEST_FETCH * 2,
            Live::new().body(url, LIMIT, &the_index()),
        )
        .await
        .expect("a caller that cannot get the turn should be refused, not queued")
        .expect_err("the turn is held for longer than anyone waits for it")
    }

    #[tokio::test(start_paused = true)]
    async fn a_caller_behind_a_fetch_that_lands_just_in_time_gets_the_index() {
        let _alone = ONE_AT_A_TIME.lock().await;
        let (urls, stub) = serve(Index::Served).await;
        let live = Live::new();
        let the_index = the_index();
        let held = TURN_TO_FETCH.lock().await;

        let the_fetch_ahead_lands = async {
            tokio::time::sleep(LONGEST_FETCH - JUST_SHORT).await;
            remember(&urls.index, &Arc::from(INDEX.as_bytes()));
            drop(held);
        };
        let (answer, ()) = futures::join!(
            live.body(&urls.index, LIMIT, &the_index),
            the_fetch_ahead_lands
        );

        let body = answer.expect("the caller ahead left the index behind inside one turn");
        assert_eq!(&*body, INDEX.as_bytes());
        assert_eq!(
            stub.asked(),
            0,
            "the index was already held when the turn came, so nothing should be fetched"
        );
    }

    const JUST_SHORT: Duration = Duration::from_millis(1);

    async fn burst(url: &str) -> Vec<Result<Body, Failure>> {
        let live = Live::new();
        let about = the_index();

        futures::future::join_all((0..BURST).map(|_| live.body(url, LIMIT, &about))).await
    }

    fn the_index() -> About<'static> {
        About {
            remember_for: Some(Duration::from_secs(600)),
            ..a_document()
        }
    }

    fn a_document() -> About<'static> {
        About {
            registry: Registry::PyPi,
            accept: None,
            compressed: false,
            remember_for: None,
            nothing_there: 404,
            blocked: &|| Failure::Internal {
                doing: "reaching the stub",
            },
            missing: &|status| Failure::Unavailable {
                registry: "pypi".to_owned(),
                status,
            },
            too_large: &|_| Failure::Internal {
                doing: "reading the stub",
            },
        }
    }

    #[derive(Clone, Copy)]
    enum Index {
        Served,
        HeldUntilAnArchiveArrives,
        RefusedWithItsOwnStatus,
    }

    struct Urls {
        index: String,
        archive: String,
    }

    #[derive(Clone)]
    struct Stub {
        index: Index,
        asked: Arc<AtomicUsize>,
        in_flight: Arc<AtomicUsize>,
        peak: Arc<AtomicUsize>,
        archive_arrived: Arc<watch::Sender<bool>>,
    }

    impl Stub {
        fn new(index: Index) -> Self {
            Self {
                index,
                asked: Arc::default(),
                in_flight: Arc::default(),
                peak: Arc::default(),
                archive_arrived: Arc::new(watch::Sender::new(false)),
            }
        }

        fn asked(&self) -> usize {
            self.asked.load(Ordering::SeqCst)
        }

        fn most_at_once(&self) -> usize {
            self.peak.load(Ordering::SeqCst)
        }
    }

    async fn serve(index: Index) -> (Urls, Stub) {
        let stub = Stub::new(index);
        let app = Router::new()
            .route("/archive", get(answer_archive))
            .fallback(answer_index)
            .with_state(stub.clone());

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("a test can bind a loopback port");
        let addr = listener
            .local_addr()
            .expect("a bound listener has an address");
        tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });

        let urls = Urls {
            index: format!("http://{addr}{}", a_path_no_other_test_has_fetched()),
            archive: format!("http://{addr}/archive"),
        };
        (urls, stub)
    }

    fn a_path_no_other_test_has_fetched() -> String {
        static STUBS: AtomicUsize = AtomicUsize::new(0);
        format!("/simple/{}/", STUBS.fetch_add(1, Ordering::SeqCst))
    }

    async fn answer_index(State(stub): State<Stub>) -> (StatusCode, &'static str) {
        let asked = stub.asked.fetch_add(1, Ordering::SeqCst) + 1;
        let now = stub.in_flight.fetch_add(1, Ordering::SeqCst) + 1;
        stub.peak.fetch_max(now, Ordering::SeqCst);

        let answer = match stub.index {
            Index::Served => (StatusCode::OK, INDEX),

            Index::HeldUntilAnArchiveArrives => {
                let mut arrived = stub.archive_arrived.subscribe();
                let _ = arrived.wait_for(|arrived| *arrived).await;
                (StatusCode::OK, INDEX)
            }

            Index::RefusedWithItsOwnStatus => (a_refusal_of_its_own(asked), "down"),
        };

        stub.in_flight.fetch_sub(1, Ordering::SeqCst);
        answer
    }

    fn a_refusal_of_its_own(asked: usize) -> StatusCode {
        let status = 500 + u16::try_from(asked).expect("a test asks a handful of times");
        StatusCode::from_u16(status).expect("a status in the 500s is a status")
    }

    async fn answer_archive(State(stub): State<Stub>) -> &'static str {
        stub.archive_arrived.send_replace(true);
        ARCHIVE
    }
}
