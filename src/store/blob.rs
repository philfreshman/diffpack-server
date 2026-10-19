use std::collections::HashSet;
use std::sync::OnceLock;
use std::time::Duration;

use reqwest::header::HeaderMap;
use reqwest::{Client, Method, RequestBuilder, Response, Url};
use serde::{Deserialize, Serialize};

use crate::error::Failure;

const API_VERSION: &str = "12";

const API_BASE: &str = "https://vercel.com/api/blob";

const STORE_ACCESS: &str = "private";

const OIDC_TOKEN: &str = "VERCEL_OIDC_TOKEN";

const STORE_ID: &str = "BLOB_STORE_ID";

const READ_WRITE_TOKEN: &str = "BLOB_READ_WRITE_TOKEN";

const PRESENTED_OIDC_TOKEN: &str = "x-vercel-oidc-token";

const ATTEMPTS: u32 = 3;

const BACKOFF: Duration = Duration::from_millis(200);

const TIMEOUT: Duration = Duration::from_secs(5);

const LONGEST_REQUEST: Duration = TIMEOUT
    .saturating_mul(ATTEMPTS)
    .saturating_add(BACKOFF.saturating_mul((1 << (ATTEMPTS - 1)) - 1));

const WHERE_IT_IS_THEN_THE_DOWNLOAD: u32 = 2;

pub(crate) const LONGEST_READ: Duration =
    LONGEST_REQUEST.saturating_mul(WHERE_IT_IS_THEN_THE_DOWNLOAD);

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Blob {
    pub(crate) pathname: String,
    pub(crate) size: u64,
    pub(crate) uploaded_at: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Download {
    download_url: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Listing {
    blobs: Vec<Blob>,
    cursor: Option<String>,
    has_more: bool,
}

#[derive(Serialize)]
struct Deletion<'a> {
    urls: &'a [&'a str],
}

pub(crate) struct Credentials {
    store_id: String,
    token: String,
}

impl Credentials {
    pub(crate) fn resolve(read: impl Fn(&str) -> Option<String>) -> Result<Self, Failure> {
        let oidc_token = read(OIDC_TOKEN);

        if let (Some(token), Some(store_id)) = (oidc_token.clone(), read(STORE_ID)) {
            return Ok(Self { store_id, token });
        }

        if let Some(token) = read(READ_WRITE_TOKEN) {
            return Self::from_read_write_token(token);
        }

        Err(Failure::Internal {
            doing: match oidc_token {
                Some(_) => {
                    "reading the blob store's credentials: \
                     an OIDC token is set but BLOB_STORE_ID is not"
                }
                None => {
                    "reading the blob store's credentials: \
                     neither VERCEL_OIDC_TOKEN, an x-vercel-oidc-token header \
                     nor BLOB_READ_WRITE_TOKEN is set"
                }
            },
        })
    }

    fn from_read_write_token(token: String) -> Result<Self, Failure> {
        let store_id = token
            .split('_')
            .nth(3)
            .filter(|id| !id.is_empty())
            .ok_or(Failure::Internal {
                doing: "reading the blob store's credentials: \
                        BLOB_READ_WRITE_TOKEN is not in the shape a store id can be read from",
            })?
            .to_owned();

        Ok(Self { store_id, token })
    }

    pub(crate) fn resolve_for(
        headers: &HeaderMap,
        read: impl Fn(&str) -> Option<String>,
    ) -> Result<Self, Failure> {
        let presented = headers
            .get(PRESENTED_OIDC_TOKEN)
            .and_then(|value| value.to_str().ok())
            .map(str::trim)
            .filter(|token| !token.is_empty())
            .map(str::to_owned);

        Self::resolve(|name| match (name, &presented) {
            (OIDC_TOKEN, Some(token)) => Some(token.clone()),
            _ => read(name),
        })
    }

    pub(crate) fn from_request(headers: &HeaderMap) -> Result<Self, Failure> {
        Self::resolve_for(headers, |name| std::env::var(name).ok())
    }

    #[cfg(test)]
    pub(crate) fn from_env() -> Result<Self, Failure> {
        Self::from_request(&HeaderMap::new())
    }
}

pub(crate) struct Api {
    base: String,
    store_id: String,
    token: String,
    backoff: Duration,
    timeout: Duration,
}

impl Api {
    pub(crate) fn at(base: impl Into<String>, store_id: &str, token: impl Into<String>) -> Self {
        Self {
            base: base.into(),
            store_id: store_id
                .strip_prefix("store_")
                .unwrap_or(store_id)
                .to_owned(),
            token: token.into(),
            backoff: BACKOFF,
            timeout: TIMEOUT,
        }
    }

    pub(crate) fn live(credentials: Credentials) -> Self {
        Self::at(API_BASE, &credentials.store_id, credentials.token)
    }

    #[cfg(test)]
    pub(crate) fn with_backoff(self, backoff: Duration) -> Self {
        Self { backoff, ..self }
    }

    #[cfg(test)]
    pub(crate) fn with_timeout(self, timeout: Duration) -> Self {
        Self { timeout, ..self }
    }

    pub(crate) async fn put(&self, pathname: &str, body: Vec<u8>) -> Result<(), Failure> {
        const DOING: &str = "writing to the blob store";

        let mut url = self.url(&format!("{}/", self.base))?;
        url.query_pairs_mut().append_pair("pathname", pathname);

        let request = self
            .request(Method::PUT, url)?
            .header("x-add-random-suffix", "0")
            .header("x-allow-overwrite", "0")
            .header("x-vercel-blob-access", STORE_ACCESS)
            .body(body);

        let response = self.send(request, DOING).await?;
        succeeded(response, DOING)?;
        Ok(())
    }

    pub(crate) async fn head(&self, pathname: &str) -> Result<Option<Blob>, Failure> {
        let mut url = self.url(&self.base)?;
        url.query_pairs_mut().append_pair("url", pathname);

        const DOING: &str = "asking the blob store for a cached result";

        let response = self.send(self.request(Method::GET, url)?, DOING).await?;

        if response.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(None);
        }

        Ok(Some(self.decode(succeeded(response, DOING)?, DOING).await?))
    }

    pub(crate) async fn read(&self, pathname: &str) -> Result<Option<Vec<u8>>, Failure> {
        const DOING: &str = "reading a cached result from the blob store";

        let mut url = self.url(&self.base)?;
        url.query_pairs_mut().append_pair("url", pathname);

        let response = self.send(self.request(Method::GET, url)?, DOING).await?;

        if response.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(None);
        }

        let at: Download = self.decode(succeeded(response, DOING)?, DOING).await?;

        let contents = client()?
            .request(Method::GET, self.url(&at.download_url)?)
            .timeout(self.timeout)
            .bearer_auth(&self.token);

        let body = succeeded(self.send(contents, DOING).await?, DOING)?
            .bytes()
            .await
            .map_err(|_| Failure::Internal { doing: DOING })?;

        Ok(Some(body.to_vec()))
    }

    pub(crate) async fn list(&self, prefix: &str) -> Result<Vec<Blob>, Failure> {
        const DOING: &str = "listing what the blob store holds";

        let mut blobs = Vec::new();
        let mut cursor: Option<String> = None;
        let mut followed = HashSet::new();

        loop {
            let mut url = self.url(&self.base)?;
            url.query_pairs_mut().append_pair("prefix", prefix);
            if let Some(cursor) = &cursor {
                url.query_pairs_mut().append_pair("cursor", cursor);
            }

            let response = self.send(self.request(Method::GET, url)?, DOING).await?;

            let page: Listing = self.decode(succeeded(response, DOING)?, DOING).await?;
            blobs.extend(page.blobs);

            match page.cursor.filter(|_| page.has_more) {
                Some(next) if followed.insert(next.clone()) => cursor = Some(next),
                _ => return Ok(blobs),
            }
        }
    }

    pub(crate) async fn delete(&self, pathnames: &[&str]) -> Result<(), Failure> {
        const DOING: &str = "deleting from the blob store";

        let url = self.url(&format!("{}/delete", self.base))?;
        let body = serde_json::to_vec(&Deletion { urls: pathnames })
            .map_err(|_| Failure::Internal { doing: DOING })?;

        let request = self
            .request(Method::POST, url)?
            .header("content-type", "application/json")
            .body(body);

        let response = self.send(request, DOING).await?;

        if response.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(());
        }

        succeeded(response, DOING)?;
        Ok(())
    }

    async fn send(
        &self,
        request: RequestBuilder,
        doing: &'static str,
    ) -> Result<Response, Failure> {
        let mut attempt = 1;
        let mut wait = self.backoff;

        loop {
            let again = request.try_clone().ok_or(Failure::Internal { doing })?;
            let outcome = again.send().await;

            let worth_retrying = match &outcome {
                Ok(response) => again_might_work(response.status()),
                Err(_) => true,
            };

            if !worth_retrying || attempt == ATTEMPTS {
                return outcome.map_err(|_| Failure::Internal { doing });
            }

            tokio::time::sleep(wait).await;
            wait *= 2;
            attempt += 1;
        }
    }

    async fn decode<T: for<'de> Deserialize<'de>>(
        &self,
        response: reqwest::Response,
        doing: &'static str,
    ) -> Result<T, Failure> {
        let body = response
            .bytes()
            .await
            .map_err(|_| Failure::Internal { doing })?;

        serde_json::from_slice(&body).map_err(|_| Failure::Internal { doing })
    }

    fn request(&self, method: Method, url: Url) -> Result<RequestBuilder, Failure> {
        Ok(client()?
            .request(method, url)
            .timeout(self.timeout)
            .header("x-api-version", API_VERSION)
            .header("x-vercel-blob-store-id", &self.store_id)
            .bearer_auth(&self.token))
    }

    fn url(&self, text: &str) -> Result<Url, Failure> {
        Url::parse(text).map_err(|_| Failure::Internal {
            doing: "building a blob store request",
        })
    }
}

fn again_might_work(status: reqwest::StatusCode) -> bool {
    status.is_server_error() || status == reqwest::StatusCode::TOO_MANY_REQUESTS
}

fn succeeded(response: Response, doing: &'static str) -> Result<Response, Failure> {
    if response.status().is_success() {
        return Ok(response);
    }
    Err(Failure::Internal { doing })
}

fn client() -> Result<&'static Client, Failure> {
    static CLIENT: OnceLock<Option<Client>> = OnceLock::new();

    CLIENT
        .get_or_init(|| {
            let _ = rustls::crypto::ring::default_provider().install_default();

            Client::builder().build().ok()
        })
        .as_ref()
        .ok_or(Failure::Internal {
            doing: "building the blob store's HTTP client",
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::collections::VecDeque;
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    use axum::body::{Body, Bytes};
    use axum::extract::{Request, State};
    use axum::http::{HeaderMap, Method, StatusCode};
    use axum::response::Response;
    use axum::Router;

    struct Received {
        method: Method,
        target: String,
        headers: HeaderMap,
        body: Bytes,
    }

    impl Received {
        fn header(&self, name: &str) -> &str {
            self.headers
                .get(name)
                .unwrap_or_else(|| panic!("the request carries no `{name}` header"))
                .to_str()
                .expect("a header this client sets is ASCII")
        }
    }

    struct Stub {
        base: String,
        received: Arc<Mutex<Vec<Received>>>,
    }

    #[derive(Clone)]
    struct Reply {
        status: StatusCode,
        body: String,
        stall: Option<Duration>,
    }

    impl Reply {
        fn ok(body: &str) -> Self {
            Self {
                status: StatusCode::OK,
                body: body.to_owned(),
                stall: None,
            }
        }

        fn stalling() -> Self {
            Self {
                stall: Some(Duration::from_secs(30)),
                ..Self::ok("{}")
            }
        }

        fn refusing(status: StatusCode, code: &str) -> Self {
            Self {
                status,
                body: format!(r#"{{"error":{{"code":"{code}"}}}}"#),
                stall: None,
            }
        }
    }

    #[derive(Clone)]
    struct Answers {
        received: Arc<Mutex<Vec<Received>>>,
        replies: Arc<Mutex<VecDeque<Reply>>>,
    }

    impl Stub {
        async fn start() -> Self {
            Self::answering(Vec::new()).await
        }

        async fn answering(replies: Vec<Reply>) -> Self {
            let received: Arc<Mutex<Vec<Received>>> = Arc::default();
            let state = Answers {
                received: Arc::clone(&received),
                replies: Arc::new(Mutex::new(replies.into())),
            };
            let app = Router::new().fallback(answer).with_state(state);

            let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
                .await
                .expect("a test can bind a loopback port");
            let addr = listener
                .local_addr()
                .expect("a bound listener has an address");
            tokio::spawn(async move {
                let _ = axum::serve(listener, app).await;
            });

            Self {
                base: format!("http://{addr}/api/blob"),
                received,
            }
        }

        fn base(&self) -> String {
            self.base.clone()
        }

        fn requests(&self) -> Vec<Received> {
            std::mem::take(
                &mut *self
                    .received
                    .lock()
                    .expect("the stub's lock is not poisoned"),
            )
        }

        fn received(&self) -> Received {
            let mut received = self
                .received
                .lock()
                .expect("the stub's lock is not poisoned");
            assert_eq!(received.len(), 1, "expected exactly one request");
            received.remove(0)
        }
    }

    async fn answer(State(state): State<Answers>, request: Request) -> Response {
        let (parts, body) = request.into_parts();
        let body = axum::body::to_bytes(body, usize::MAX)
            .await
            .expect("a test request body is small enough to buffer");

        state
            .received
            .lock()
            .expect("the stub's lock is not poisoned")
            .push(Received {
                method: parts.method,
                target: parts
                    .uri
                    .path_and_query()
                    .map(ToString::to_string)
                    .unwrap_or_default(),
                headers: parts.headers,
                body,
            });

        let reply = state
            .replies
            .lock()
            .expect("the stub's lock is not poisoned")
            .pop_front()
            .unwrap_or_else(|| Reply::ok("{}"));

        if let Some(stall) = reply.stall {
            tokio::time::sleep(stall).await;
        }

        Response::builder()
            .status(reply.status)
            .body(Body::from(reply.body))
            .expect("a 200 with a JSON body is a valid response")
    }

    #[tokio::test]
    async fn a_put_carries_its_pathname_in_the_query_and_its_body_in_the_body() {
        let stub = Stub::start().await;
        let api = Api::at(stub.base(), "a-test-store", "a-token");

        api.put("diffs/v1/abc/meta.json", b"{\"cached\":true}".to_vec())
            .await
            .expect("the stub answers the way the API does");

        let request = stub.received();
        assert_eq!(request.method, Method::PUT);
        assert_eq!(
            request.target,
            "/api/blob/?pathname=diffs%2Fv1%2Fabc%2Fmeta.json"
        );
        assert_eq!(request.body.as_ref(), b"{\"cached\":true}");
    }

    #[tokio::test]
    async fn every_request_names_the_api_version_the_store_and_the_bearer() {
        let stub = Stub::start().await;
        let api = Api::at(stub.base(), "a-test-store", "a-token");

        api.put("diffs/v1/abc/meta.json", b"{}".to_vec())
            .await
            .expect("the stub answers the way the API does");

        let request = stub.received();
        assert_eq!(request.header("x-api-version"), "12");
        assert_eq!(request.header("x-vercel-blob-store-id"), "a-test-store");
        assert_eq!(request.header("authorization"), "Bearer a-token");
    }

    #[tokio::test]
    async fn a_store_id_reaches_the_header_without_the_prefix_it_is_provisioned_with() {
        let stub = Stub::start().await;
        let api = Api::at(stub.base(), "store_a-test-store", "a-token");

        api.put("diffs/v1/abc/meta.json", b"{}".to_vec())
            .await
            .expect("the stub answers the way the API does");

        assert_eq!(
            stub.received().header("x-vercel-blob-store-id"),
            "a-test-store"
        );
    }

    #[tokio::test]
    async fn a_write_lands_at_the_pathname_it_was_given_and_does_not_replace_what_is_there() {
        let stub = Stub::start().await;
        let api = Api::at(stub.base(), "a-test-store", "a-token");

        api.put("diffs/v1/abc/meta.json", b"{}".to_vec())
            .await
            .expect("the stub answers the way the API does");

        let request = stub.received();
        assert_eq!(request.header("x-add-random-suffix"), "0");
        assert_eq!(request.header("x-allow-overwrite"), "0");
    }

    #[tokio::test]
    async fn a_write_declares_the_private_access_the_store_is_provisioned_with() {
        let stub = Stub::start().await;
        let api = Api::at(stub.base(), "a-test-store", "a-token");

        api.put("diffs/v1/abc/meta.json", b"{}".to_vec())
            .await
            .expect("the stub answers the way the API does");

        assert_eq!(
            stub.received().header("x-vercel-blob-access"),
            "private",
            "the store is private, and it refuses a write that asks for public access"
        );
    }

    #[tokio::test]
    async fn a_head_answers_with_the_size_and_the_age_of_a_blob_that_is_there() {
        let stub = Stub::answering(vec![Reply::ok(
            r#"{"pathname":"diffs/v1/abc/meta.json","size":1234,
                "uploadedAt":"2026-09-21T10:00:00.000Z"}"#,
        )])
        .await;
        let api = Api::at(stub.base(), "a-test-store", "a-token");

        let found = api
            .head("diffs/v1/abc/meta.json")
            .await
            .expect("the stub answers the way the API does")
            .expect("the API answered with a blob");

        assert_eq!(found.pathname, "diffs/v1/abc/meta.json");
        assert_eq!(found.size, 1234);
        assert_eq!(found.uploaded_at, "2026-09-21T10:00:00.000Z");

        let request = stub.received();
        assert_eq!(request.method, Method::GET);
        assert_eq!(request.target, "/api/blob?url=diffs%2Fv1%2Fabc%2Fmeta.json");
    }

    #[tokio::test]
    async fn a_head_of_a_blob_the_store_does_not_hold_is_a_miss_and_not_a_failure() {
        let stub = Stub::answering(vec![Reply::refusing(StatusCode::NOT_FOUND, "not_found")]).await;
        let api = Api::at(stub.base(), "a-test-store", "a-token");

        let found = api
            .head("diffs/v1/nothing-here/meta.json")
            .await
            .expect("a cold cache is not a failure");

        assert!(found.is_none(), "got {found:?}");
    }

    #[tokio::test]
    async fn a_list_follows_the_cursor_until_the_store_says_there_is_no_more() {
        let stub = Stub::answering(vec![
            Reply::ok(
                r#"{"blobs":[{"pathname":"diffs/v1/aaa/meta.json","size":10,
                    "uploadedAt":"2026-09-21T10:00:00.000Z"}],
                    "cursor":"the-second-page","hasMore":true}"#,
            ),
            Reply::ok(
                r#"{"blobs":[{"pathname":"diffs/v1/bbb/meta.json","size":20,
                    "uploadedAt":"2026-09-21T11:00:00.000Z"}],
                    "hasMore":false}"#,
            ),
        ])
        .await;
        let api = Api::at(stub.base(), "a-test-store", "a-token");

        let blobs = api
            .list("diffs/v1/")
            .await
            .expect("the stub answers the way the API does");

        let paths: Vec<&str> = blobs.iter().map(|blob| blob.pathname.as_str()).collect();
        assert_eq!(paths, ["diffs/v1/aaa/meta.json", "diffs/v1/bbb/meta.json"]);
        assert_eq!(blobs[1].size, 20);
        assert_eq!(blobs[1].uploaded_at, "2026-09-21T11:00:00.000Z");

        let requests = stub.requests();
        assert_eq!(requests.len(), 2, "the walk should have asked twice");
        assert_eq!(requests[0].target, "/api/blob?prefix=diffs%2Fv1%2F");
        assert_eq!(
            requests[1].target,
            "/api/blob?prefix=diffs%2Fv1%2F&cursor=the-second-page"
        );
    }

    #[tokio::test]
    async fn a_list_the_store_cannot_say_where_to_resume_ends_rather_than_asking_again() {
        let stub = Stub::answering(vec![Reply::ok(
            r#"{"blobs":[{"pathname":"diffs/v1/aaa/meta.json","size":10,
                "uploadedAt":"2026-09-21T10:00:00.000Z"}],"hasMore":true}"#,
        )])
        .await;
        let api = Api::at(stub.base(), "a-test-store", "a-token");

        let blobs = api
            .list("diffs/v1/")
            .await
            .expect("a page with nowhere to resume from is the end of the walk");

        assert_eq!(blobs.len(), 1, "got {blobs:?}");
        assert_eq!(
            stub.requests().len(),
            1,
            "there was no cursor to follow, so there was nothing to ask twice"
        );
    }

    #[tokio::test]
    async fn a_list_sent_back_to_a_cursor_it_already_followed_ends_rather_than_asking_again() {
        let stub = Stub::answering(vec![
            Reply::ok(
                r#"{"blobs":[{"pathname":"diffs/v1/aaa/meta.json","size":10,
                    "uploadedAt":"2026-09-21T10:00:00.000Z"}],
                    "cursor":"the-second-page","hasMore":true}"#,
            ),
            Reply::ok(
                r#"{"blobs":[{"pathname":"diffs/v1/bbb/meta.json","size":20,
                    "uploadedAt":"2026-09-21T11:00:00.000Z"}],
                    "cursor":"the-third-page","hasMore":true}"#,
            ),
            Reply::ok(
                r#"{"blobs":[{"pathname":"diffs/v1/ccc/meta.json","size":30,
                    "uploadedAt":"2026-09-21T12:00:00.000Z"}],
                    "cursor":"the-second-page","hasMore":true}"#,
            ),
        ])
        .await;
        let api = Api::at(stub.base(), "a-test-store", "a-token");

        let blobs = api
            .list("diffs/v1/")
            .await
            .expect("a cursor already followed is the end of the walk");

        let paths: Vec<&str> = blobs.iter().map(|blob| blob.pathname.as_str()).collect();
        assert_eq!(
            paths,
            [
                "diffs/v1/aaa/meta.json",
                "diffs/v1/bbb/meta.json",
                "diffs/v1/ccc/meta.json"
            ]
        );

        let targets: Vec<String> = stub
            .requests()
            .into_iter()
            .map(|request| request.target)
            .collect();
        assert_eq!(
            targets,
            [
                "/api/blob?prefix=diffs%2Fv1%2F",
                "/api/blob?prefix=diffs%2Fv1%2F&cursor=the-second-page",
                "/api/blob?prefix=diffs%2Fv1%2F&cursor=the-third-page",
            ],
            "each cursor should have been asked for once"
        );
    }

    #[tokio::test]
    async fn a_delete_takes_every_path_it_is_given_in_one_request() {
        let stub = Stub::start().await;
        let api = Api::at(stub.base(), "a-test-store", "a-token");

        api.delete(&["diffs/v1/aaa/meta.json", "diffs/v1/aaa/patches.json"])
            .await
            .expect("the stub answers the way the API does");

        let request = stub.received();
        assert_eq!(request.method, Method::POST);
        assert_eq!(request.target, "/api/blob/delete");
        assert_eq!(request.header("content-type"), "application/json");

        let body: serde_json::Value =
            serde_json::from_slice(&request.body).expect("the body this client sends is JSON");
        assert_eq!(
            body,
            serde_json::json!({
                "urls": ["diffs/v1/aaa/meta.json", "diffs/v1/aaa/patches.json"]
            })
        );
    }

    #[tokio::test]
    async fn deleting_a_blob_that_is_already_gone_is_not_a_failure() {
        let stub = Stub::answering(vec![Reply::refusing(StatusCode::NOT_FOUND, "not_found")]).await;
        let api = Api::at(stub.base(), "a-test-store", "a-token");

        api.delete(&["diffs/v1/evicted-already/meta.json"])
            .await
            .expect("a blob that is already gone is the outcome that was asked for");
    }

    #[tokio::test]
    async fn a_write_the_store_refuses_is_a_failure_and_not_a_silent_success() {
        let stub = Stub::answering(vec![Reply::refusing(StatusCode::FORBIDDEN, "forbidden")]).await;
        let api = Api::at(stub.base(), "a-test-store", "a-token");

        api.put("diffs/v1/abc/meta.json", b"{}".to_vec())
            .await
            .expect_err("a write the store refused is not a write");
    }

    #[tokio::test]
    async fn a_read_presents_the_token_where_the_private_blob_is_served() {
        let blobs = Stub::answering(vec![Reply::ok(r#"{"cached":true}"#)]).await;
        let served_at = format!("{}/diffs/v1/abc/meta.json?download=1", blobs.base());
        let api_stub = Stub::answering(vec![Reply::ok(&format!(
            r#"{{"downloadUrl":"{served_at}"}}"#
        ))])
        .await;
        let api = Api::at(api_stub.base(), "a-test-store", "a-token");

        let body = api
            .read("diffs/v1/abc/meta.json")
            .await
            .expect("both stubs answer the way the store does")
            .expect("the store said where the blob is");

        assert_eq!(body, br#"{"cached":true}"#);
        assert_eq!(
            blobs.received().header("authorization"),
            "Bearer a-token",
            "a private blob is served only to a request that carries the store's token"
        );
    }

    #[tokio::test]
    async fn a_read_the_store_refuses_is_a_failure_and_not_a_miss() {
        let stub = Stub::answering(vec![Reply::refusing(StatusCode::FORBIDDEN, "forbidden")]).await;
        let api = Api::at(stub.base(), "a-test-store", "a-token");

        api.read("diffs/v1/abc/meta.json")
            .await
            .expect_err("a read the store refused is not a miss");
    }

    #[tokio::test]
    async fn a_request_the_store_could_not_answer_is_tried_again() {
        let stub = Stub::answering(vec![
            Reply::refusing(StatusCode::SERVICE_UNAVAILABLE, "service_unavailable"),
            Reply::ok("{}"),
        ])
        .await;
        let api =
            Api::at(stub.base(), "a-test-store", "a-token").with_backoff(Duration::from_millis(1));

        api.put("diffs/v1/abc/meta.json", b"{}".to_vec())
            .await
            .expect("the second attempt is the one that worked");

        assert_eq!(
            stub.requests().len(),
            2,
            "a store that answered `503` should have been asked again"
        );
    }

    #[tokio::test]
    async fn a_store_that_asked_for_a_moment_is_given_one() {
        let stub = Stub::answering(vec![
            Reply::refusing(StatusCode::TOO_MANY_REQUESTS, "too_many_requests"),
            Reply::ok("{}"),
        ])
        .await;
        let api =
            Api::at(stub.base(), "a-test-store", "a-token").with_backoff(Duration::from_millis(1));

        api.put("diffs/v1/abc/meta.json", b"{}".to_vec())
            .await
            .expect("the second attempt is the one that worked");

        assert_eq!(
            stub.requests().len(),
            2,
            "a store that answered `429` should have been asked again"
        );
    }

    #[tokio::test]
    async fn a_store_that_never_answers_is_given_up_on_rather_than_waited_out() {
        let stub = Stub::answering(vec![
            Reply::stalling(),
            Reply::stalling(),
            Reply::stalling(),
        ])
        .await;
        let api = Api::at(stub.base(), "a-test-store", "a-token")
            .with_backoff(Duration::from_millis(1))
            .with_timeout(Duration::from_millis(50));

        api.head("diffs/v1/abc/meta.json")
            .await
            .expect_err("a store that never answered did not answer");
    }

    #[tokio::test]
    async fn a_refusal_the_store_would_only_repeat_is_not_tried_again() {
        let stub = Stub::answering(vec![Reply::refusing(StatusCode::NOT_FOUND, "not_found")]).await;
        let api =
            Api::at(stub.base(), "a-test-store", "a-token").with_backoff(Duration::from_millis(1));

        api.head("diffs/v1/nothing-here/meta.json")
            .await
            .expect("a cold cache is not a failure");

        assert_eq!(
            stub.requests().len(),
            1,
            "a `404` is an answer, so it should have been asked once"
        );
    }

    #[tokio::test]
    async fn no_failure_this_client_builds_carries_the_token() {
        let token = ["vercel", "blob", "rw", "A1b2C3d4E5f6G7h8i9J0kL1mN2oP3qR4"].join("_");

        let stub = Stub::answering(vec![Reply {
            status: StatusCode::FORBIDDEN,
            body: format!(r#"{{"error":{{"code":"forbidden","message":"token {token} denied"}}}}"#),
            stall: None,
        }])
        .await;
        let api = Api::at(stub.base(), "a-test-store", token.clone())
            .with_backoff(Duration::from_millis(1));

        let failure = api
            .put("diffs/v1/abc/meta.json", b"{}".to_vec())
            .await
            .expect_err("the store refused this write");

        let logged = format!("{failure:?}");
        let sent = match failure.respond() {
            Ok(answer) => format!("{answer:?}"),
            Err(error) => format!("{error:?}"),
        };

        for rendering in [&logged, &sent] {
            assert!(
                !rendering.contains(&token),
                "the token survived into a failure: {rendering}"
            );
        }
    }

    fn env(variables: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
        let variables: Vec<(String, String)> = variables
            .iter()
            .map(|(name, value)| ((*name).to_owned(), (*value).to_owned()))
            .collect();

        move |wanted| {
            variables
                .iter()
                .find(|(name, _)| name == wanted)
                .map(|(_, value)| value.clone())
        }
    }

    #[test]
    fn an_oidc_token_beside_a_store_id_is_what_authorises_a_request() {
        let credentials = Credentials::resolve(env(&[
            ("VERCEL_OIDC_TOKEN", "an-oidc-token"),
            ("BLOB_STORE_ID", "store_a-test-store"),
        ]))
        .expect("this is the pair this deployment is given");

        assert_eq!(credentials.store_id, "store_a-test-store");
        assert_eq!(credentials.token, "an-oidc-token");
    }

    #[test]
    fn a_token_presented_with_the_request_is_the_one_the_store_is_asked_with() {
        let mut headers = HeaderMap::new();
        headers.insert(
            "x-vercel-oidc-token",
            "a-request-token"
                .parse()
                .expect("a test token is a header value"),
        );

        let credentials = Credentials::resolve_for(
            &headers,
            env(&[
                ("BLOB_STORE_ID", "store_a-test-store"),
                ("VERCEL_OIDC_TOKEN", "a-build-time-token"),
            ]),
        )
        .expect("a presented token with the store id is credentials");

        assert_eq!(
            credentials.token, "a-request-token",
            "a deployed function is given its token with each request, and the \
             environment only has one at build time"
        );
        assert_eq!(credentials.store_id, "store_a-test-store");
    }

    #[test]
    fn a_request_that_presents_no_token_falls_back_to_the_environment() {
        let credentials = Credentials::resolve_for(
            &HeaderMap::new(),
            env(&[
                ("BLOB_STORE_ID", "store_a-test-store"),
                ("VERCEL_OIDC_TOKEN", "a-local-token"),
            ]),
        )
        .expect("the environment's pair is credentials on its own");

        assert_eq!(credentials.token, "a-local-token");
    }

    #[test]
    fn a_presented_token_without_a_store_id_leaves_the_read_write_token_to_answer() {
        let read_write = [
            "vercel",
            "blob",
            "rw",
            "a-test-store",
            "A1b2C3d4E5f6G7h8i9J0kL",
        ]
        .join("_");

        let credentials = Credentials::resolve_for(
            &presenting("a-request-token"),
            env(&[("BLOB_READ_WRITE_TOKEN", read_write.as_str())]),
        )
        .expect("a deployment given only a read-write token still has a store");

        assert_eq!(
            credentials.token, read_write,
            "Vercel presents a token on every request once OIDC is on, and a \
             deployment that never set BLOB_STORE_ID did not mean to use it"
        );
    }

    #[test]
    fn a_blank_presented_token_is_no_token() {
        let credentials = Credentials::resolve_for(
            &presenting("  "),
            env(&[
                ("BLOB_STORE_ID", "store_a-test-store"),
                ("VERCEL_OIDC_TOKEN", "a-local-token"),
            ]),
        )
        .expect("the environment's pair is credentials on its own");

        assert_eq!(credentials.token, "a-local-token");
    }

    fn presenting(token: &str) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(
            "x-vercel-oidc-token",
            token.parse().expect("a test token is a header value"),
        );
        headers
    }

    #[test]
    fn a_read_write_token_names_the_store_it_is_for() {
        let token = [
            "vercel",
            "blob",
            "rw",
            "a-test-store",
            "A1b2C3d4E5f6G7h8i9J0kL",
        ]
        .join("_");

        let credentials = Credentials::resolve(env(&[("BLOB_READ_WRITE_TOKEN", token.as_str())]))
            .expect("a read-write token is credentials on its own");

        assert_eq!(credentials.store_id, "a-test-store");
        assert_eq!(credentials.token, token);
    }

    #[test]
    fn a_deployment_missing_a_credential_fails_naming_the_variable_to_set() {
        let cases: [(&[(&str, &str)], &str); 3] = [
            (&[("VERCEL_OIDC_TOKEN", "an-oidc-token")], "BLOB_STORE_ID"),
            (&[], "VERCEL_OIDC_TOKEN"),
            (
                &[("BLOB_READ_WRITE_TOKEN", "not-a-token")],
                "BLOB_READ_WRITE_TOKEN",
            ),
        ];

        for (variables, named) in cases {
            let Err(failure) = Credentials::resolve(env(variables)) else {
                panic!("`{named}` is missing, so these are not credentials");
            };

            let message = match failure.respond() {
                Ok(answer) => format!("{answer:?}"),
                Err(error) => error.message.into_owned(),
            };

            assert!(
                message.contains(named),
                "the failure should name `{named}`, and says: {message}"
            );
        }
    }

    #[tokio::test]
    #[ignore = "networked: writes to this project's Vercel Blob store"]
    async fn the_real_store_answers_the_four_requests_this_client_makes() {
        let credentials = Credentials::from_env()
            .expect("VERCEL_OIDC_TOKEN with BLOB_STORE_ID, or BLOB_READ_WRITE_TOKEN");
        let api = Api::live(credentials);

        let run = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("the clock is set after 1970")
            .as_nanos();
        let prefix = format!("tests/{run}/");
        let pathname = format!("{prefix}meta.json");
        let body = br#"{"cached":true}"#.to_vec();

        api.put(&pathname, body.clone())
            .await
            .expect("the store takes a write at a pathname of ours");

        let found = api
            .head(&pathname)
            .await
            .expect("the store answers about a blob it holds")
            .expect("the blob written a moment ago is there");

        assert_eq!(found.pathname, pathname);
        assert_eq!(found.size, body.len() as u64);
        assert!(
            !found.uploaded_at.is_empty(),
            "eviction orders by this, so a blob without one is not evictable"
        );

        let listed = api
            .list(&prefix)
            .await
            .expect("the store lists what is under a prefix");
        let paths: Vec<&str> = listed.iter().map(|blob| blob.pathname.as_str()).collect();
        assert_eq!(paths, [pathname.as_str()]);

        api.delete(&[pathname.as_str()])
            .await
            .expect("the store deletes what it was given");

        api.delete(&[pathname.as_str()])
            .await
            .expect("deleting a blob that is already gone is the outcome asked for");
    }
}
