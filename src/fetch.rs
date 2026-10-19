use std::sync::OnceLock;
use std::time::Duration;

use reqwest::redirect::Policy;
use reqwest::{Client, Response, StatusCode};
use tokio::sync::Semaphore;

use crate::error::{self, Failure};
use crate::registry::{self, Registry};

pub const DOWNLOADS_AT_ONCE: usize = 4;

const SLOT_WAIT: Duration = error::UPSTREAM_TIMEOUT;

pub const LONGEST_FETCH: Duration = SLOT_WAIT.saturating_add(error::UPSTREAM_TIMEOUT);

static DOWNLOADS: Semaphore = Semaphore::const_new(DOWNLOADS_AT_ONCE);

const USER_AGENT: &str = concat!(
    "diffpack-server/",
    env!("CARGO_PKG_VERSION"),
    " (+https://github.com/philfreshman/diffpack-server)",
);

pub struct About<'a> {
    pub registry: Registry,

    pub accept: Option<&'static str>,

    pub compressed: bool,

    pub missing: &'a (dyn Fn(u16) -> Failure + Send + Sync),

    pub too_large: &'a (dyn Fn(u64) -> Failure + Send + Sync),
}

pub async fn bytes(url: &str, limit: u64, about: &About<'_>) -> Result<Vec<u8>, Failure> {
    let _slot = match tokio::time::timeout(SLOT_WAIT, DOWNLOADS.acquire()).await {
        Ok(Ok(slot)) => slot,

        Err(_) => return Err(Failure::Busy { waited: SLOT_WAIT }),

        Ok(Err(_)) => {
            return Err(Failure::Internal {
                doing: "waiting for a download slot",
            })
        }
    };

    let client = client()?;
    let name = about.registry.name();

    let mut request = client.get(url);
    if let Some(accept) = about.accept {
        request = request.header(reqwest::header::ACCEPT, accept);
    }
    if !about.compressed {
        request = request.header(reqwest::header::ACCEPT_ENCODING, "identity");
    }

    let response = error::within_budget(name, request.send())
        .await?
        .map_err(|cause| failed(cause, name))?;

    let response = success(response, about)?;
    read_within(response, limit, name, about).await
}

fn success(response: Response, about: &About<'_>) -> Result<Response, Failure> {
    let status = response.status();
    if status.is_success() {
        return Ok(response);
    }

    Err(match status {
        StatusCode::NOT_FOUND
        | StatusCode::FORBIDDEN
        | StatusCode::METHOD_NOT_ALLOWED
        | StatusCode::GONE => (about.missing)(status.as_u16()),

        StatusCode::TOO_MANY_REQUESTS => Failure::RateLimited {
            registry: about.registry.name().to_owned(),
            retry_after: retry_after(&response),
        },

        other => Failure::Unavailable {
            registry: about.registry.name().to_owned(),
            status: other.as_u16(),
        },
    })
}

fn retry_after(response: &Response) -> Option<std::time::Duration> {
    response
        .headers()
        .get(reqwest::header::RETRY_AFTER)?
        .to_str()
        .ok()?
        .trim()
        .parse::<u64>()
        .ok()
        .map(std::time::Duration::from_secs)
}

async fn read_within(
    mut response: Response,
    limit: u64,
    registry: &str,
    about: &About<'_>,
) -> Result<Vec<u8>, Failure> {
    if !about.compressed {
        if let Some(declared) = response.content_length() {
            if declared > limit {
                return Err((about.too_large)(declared));
            }
        }
    }

    let mut body: Vec<u8> = Vec::new();
    while let Some(chunk) = error::within_budget(registry, response.chunk())
        .await?
        .map_err(|cause| failed(cause, registry))?
    {
        let weight = body.len() as u64 + chunk.len() as u64;
        if weight > limit {
            return Err((about.too_large)(weight));
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

fn failed(cause: reqwest::Error, registry: &str) -> Failure {
    if cause.is_timeout() {
        return Failure::TimedOut {
            registry: registry.to_owned(),
            waited: error::UPSTREAM_TIMEOUT,
        };
    }
    Failure::Unreachable {
        registry: registry.to_owned(),
    }
}

fn client() -> Result<&'static Client, Failure> {
    static CLIENT: OnceLock<Option<Client>> = OnceLock::new();

    CLIENT
        .get_or_init(|| {
            let _ = rustls::crypto::ring::default_provider().install_default();

            Client::builder()
                .user_agent(USER_AGENT)
                .timeout(error::UPSTREAM_TIMEOUT)
                .redirect(Policy::custom(|attempt| {
                    if registry::allows(attempt.url().as_str()) {
                        attempt.follow()
                    } else {
                        attempt.stop()
                    }
                }))
                .build()
                .ok()
        })
        .as_ref()
        .ok_or(Failure::Internal {
            doing: "building the HTTP client",
        })
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;
    use std::time::Duration;

    use axum::extract::State;
    use axum::Router;

    pub(crate) static ONE_AT_A_TIME: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

    const HELD: Duration = Duration::from_millis(100);

    #[derive(Clone, Default)]
    struct InFlight {
        now: Arc<AtomicUsize>,
        peak: Arc<AtomicUsize>,
    }

    impl InFlight {
        fn arrived(&self) {
            let now = self.now.fetch_add(1, Ordering::SeqCst) + 1;
            self.peak.fetch_max(now, Ordering::SeqCst);
        }

        fn left(&self) {
            self.now.fetch_sub(1, Ordering::SeqCst);
        }

        fn peak(&self) -> usize {
            self.peak.load(Ordering::SeqCst)
        }
    }

    async fn stub() -> (String, InFlight) {
        let in_flight = InFlight::default();
        let app = Router::new().fallback(hold).with_state(in_flight.clone());

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("a test can bind a loopback port");
        let addr = listener
            .local_addr()
            .expect("a bound listener has an address");
        tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });

        (format!("http://{addr}/archive"), in_flight)
    }

    async fn hold(State(in_flight): State<InFlight>) -> &'static str {
        in_flight.arrived();
        tokio::time::sleep(HELD).await;
        in_flight.left();

        "held"
    }

    fn about() -> About<'static> {
        About {
            registry: Registry::Npm,
            accept: None,
            compressed: false,
            missing: &|_| Failure::Unreachable {
                registry: "npm".to_owned(),
            },
            too_large: &|_| Failure::Unreachable {
                registry: "npm".to_owned(),
            },
        }
    }

    #[tokio::test]
    async fn no_more_bodies_arrive_at_once_than_the_cap_allows() {
        let _turn = ONE_AT_A_TIME.lock().await;
        let (url, in_flight) = stub().await;

        let about = about();
        let asked = (0..DOWNLOADS_AT_ONCE * 2 + 1).map(|_| bytes(&url, 1024, &about));
        let answers = futures::future::join_all(asked).await;

        assert!(
            answers.iter().all(Result::is_ok),
            "the stub answers every request, got {answers:?}"
        );
        assert!(
            in_flight.peak() <= DOWNLOADS_AT_ONCE,
            "{} bodies were on their way in at once, which is past the cap of \
             {DOWNLOADS_AT_ONCE}",
            in_flight.peak()
        );
    }

    #[tokio::test(start_paused = true)]
    async fn a_call_that_waits_out_its_turn_for_a_slot_is_refused() {
        let _turn = ONE_AT_A_TIME.lock().await;

        let mut held = Vec::new();
        for _ in 0..DOWNLOADS_AT_ONCE {
            held.push(
                DOWNLOADS
                    .acquire()
                    .await
                    .expect("the download slots are never closed"),
            );
        }

        let refused = tokio::time::timeout(SLOT_WAIT * 2, bytes(UNSERVED, 1024, &about()))
            .await
            .expect("a call that cannot get a slot should be refused, not queued");

        assert_eq!(
            refused.as_ref().err().map(Failure::kind),
            Some("busy"),
            "the wait was this server's queue and not a registry's silence, got {refused:?}"
        );

        drop(held);
    }

    const UNSERVED: &str = "http://127.0.0.1:1/archive";

    #[tokio::test]
    async fn both_archives_of_one_comparison_arrive_together() {
        let _turn = ONE_AT_A_TIME.lock().await;
        let (url, in_flight) = stub().await;
        let about = about();

        let both = futures::join!(bytes(&url, 1024, &about), bytes(&url, 1024, &about));

        assert!(
            both.0.is_ok() && both.1.is_ok(),
            "the stub answers both requests, got {both:?}"
        );
        assert_eq!(
            in_flight.peak(),
            2,
            "a comparison asks for both versions at once, and the cap has to \
             let it"
        );
    }

    async fn answering(status: axum::http::StatusCode) -> String {
        let app = Router::new().fallback(move || async move { status });

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("a test can bind a loopback port");
        let addr = listener
            .local_addr()
            .expect("a bound listener has an address");
        tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });

        format!("http://{addr}/types%2Fnode")
    }

    #[tokio::test]
    async fn a_url_the_registry_will_not_serve_a_get_at_names_nothing() {
        let _turn = ONE_AT_A_TIME.lock().await;
        let url = answering(axum::http::StatusCode::METHOD_NOT_ALLOWED).await;
        let about = About {
            missing: &|_| Failure::NoSuchPackage {
                registry: "npm".to_owned(),
                package: "types/node".to_owned(),
            },
            ..about()
        };

        let refused = bytes(&url, 1024, &about).await;

        assert_eq!(
            refused.as_ref().err().map(Failure::kind),
            Some("no_such_package"),
            "npm answers 405 to an unscoped name with a slash in it; this server \
             only ever sends a GET, so asking again cannot help and the name is \
             what to correct, got {refused:?}"
        );
    }

    #[tokio::test]
    async fn a_fetch_that_failed_gives_its_slot_back() {
        let _turn = ONE_AT_A_TIME.lock().await;
        let (url, _) = stub().await;
        let about = about();

        let refused =
            futures::future::join_all((0..DOWNLOADS_AT_ONCE + 1).map(|_| bytes(&url, 0, &about)))
                .await;

        assert!(
            refused.iter().all(Result::is_err),
            "a body of any size is over a limit of none, got {refused:?}"
        );
        assert_eq!(
            DOWNLOADS.available_permits(),
            DOWNLOADS_AT_ONCE,
            "every slot a refused fetch took should be back"
        );
    }
}
