//! The Vercel Blob client.
//!
//! Five operations, because five is what the cache needs: write a blob, ask
//! whether one is there, read one back, list what is under a prefix, delete
//! several at once. It is not a general client for the service and is not meant to
//! become one — [ADR 0003](../docs/adr/0003-the-cache-seam-is-a-store.md)
//! rejects exposing these verbs to callers, so what leaves this module is
//! `DiffStore`'s answers and never a pathname.
//!
//! # Why this is hand-rolled
//!
//! There is no usable Rust client for Vercel Blob: `vercel_blob` is 0.1.0
//! from October 2023 and predates the current API base URL. So this is
//! written against what `@vercel/blob` sends, read from its published source
//! rather than from documentation — the API is private and has no other
//! specification.

use std::sync::OnceLock;
use std::time::Duration;

use reqwest::{Client, Method, RequestBuilder, Response, Url};
use serde::{Deserialize, Serialize};

use crate::error::Failure;

/// The API revision this client is written against.
///
/// It decides the shape of what comes back, so it is not a number to bump
/// idly: the parsing below is written for this version and for no other.
const API_VERSION: &str = "12";

/// Where the API lives. No trailing slash: the operations below append one
/// where the API wants one and not where it does not.
const API_BASE: &str = "https://vercel.com/api/blob";

/// How many times one request is sent before this client gives up.
///
/// Three, not the ten `@vercel/blob` defaults to. That client serves a
/// user's upload, where a retry is the difference between working and not;
/// this one serves a cache, where giving up costs a recomputed diff and
/// nothing else. Ten attempts against a store that is genuinely down would
/// spend the function's time budget on a result it was going to compute
/// anyway.
const ATTEMPTS: u32 = 3;

/// How long the second attempt waits, doubling for each one after it.
///
/// Small, for the same reason there are three of them: the whole retry
/// budget here is under a second, because a caller is a diff that already
/// has an answer and is only trying to remember it.
const BACKOFF: Duration = Duration::from_millis(200);

/// How long one attempt may take before it is abandoned.
///
/// Every request needs one or a store that accepts a connection and then
/// says nothing holds this function until the platform kills it — with no
/// answer to the caller and nothing in the log about which of the two was
/// waiting. That is the failure [ADR
/// 0001](../docs/adr/0001-the-archive-seam-is-a-filemap.md) is about, and a
/// second client is a second place to have forgotten it.
///
/// Shorter than [`crate::error::UPSTREAM_TIMEOUT`], which is what this server
/// waits on a registry, because the two are waiting for different things. A
/// registry holds the only copy of what is being asked for, so waiting is the
/// whole of the remedy. The store holds a copy of an answer this function can
/// work out again, so three attempts and the sleeps between them still leave
/// room to do the work the cache was there to save.
const TIMEOUT: Duration = Duration::from_secs(5);

/// One blob, as the store describes it — `CONTEXT.md`'s noun exactly.
///
/// Three fields out of the many the API returns, because three is what the
/// cache is built on: the pathname says which entry a blob belongs to, the
/// size is what the 256 MB budget is counted in, and `uploaded_at` is the
/// order eviction runs in. There is no separate index to keep in step with
/// the store, which is the point — these come back from a `list` for free.
///
/// `uploaded_at` stays the string the API sent. It is ISO-8601 in UTC to
/// the millisecond, a format whose lexical order is its chronological
/// order, so eviction sorts on it without this module taking a dependency
/// on a calendar.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Blob {
    pub(crate) pathname: String,
    pub(crate) size: u64,
    pub(crate) uploaded_at: String,
}

/// Where the store says a blob's bytes can be fetched from.
///
/// Its own type rather than a fourth field on [`Blob`]. The three fields
/// there are what the cache is built on and outlive any one request; this is
/// a detail of one read, and putting it beside them would invite a caller to
/// keep it.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Download {
    download_url: String,
}

/// One page of a listing, as the API answers it.
///
/// Not called a Page: that word is `src/page.rs`'s, for as much of an answer
/// as fits under the response ceiling, and this is the store's own paging of
/// its own contents on the way in. Nothing outside this module sees either
/// one of these, which is exactly when two meanings of a word get written
/// down by accident.
///
/// Both fields end the walk, because either one alone can be wrong in a way
/// the other catches. `has_more` is the store saying it is finished, and
/// `cursor` is somewhere to resume from — so a page claiming more without
/// naming where would otherwise be a request for the first page again,
/// forever, and a page naming a cursor it has already given back is the same
/// loop by another route.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Listing {
    blobs: Vec<Blob>,
    cursor: Option<String>,
    has_more: bool,
}

/// The body of a delete, which is the only request here that has one shape
/// rather than a string.
#[derive(Serialize)]
struct Deletion<'a> {
    /// The API's name for the field, and it takes pathnames as readily as
    /// URLs.
    urls: &'a [&'a str],
}

/// What a request to the store is authorised with.
///
/// Two fields rather than one, because the credential this deployment has
/// does not carry the store it is for. An OIDC token is minted by the
/// platform for the whole project, so the store id arrives beside it as its
/// own variable — which is also why it is sent as its own header.
pub(crate) struct Credentials {
    store_id: String,
    token: String,
}

impl Credentials {
    /// The credentials in `read`, or a failure naming what is not there.
    ///
    /// A lookup rather than the process environment, so that what this
    /// resolves is a question with an argument instead of a global read.
    pub(crate) fn resolve(read: impl Fn(&str) -> Option<String>) -> Result<Self, Failure> {
        if let Some(token) = read("VERCEL_OIDC_TOKEN") {
            let store_id = read("BLOB_STORE_ID").ok_or(Failure::Internal {
                doing: "reading the blob store's credentials: \
                        an OIDC token is set but BLOB_STORE_ID is not",
            })?;
            return Ok(Self { store_id, token });
        }

        if let Some(token) = read("BLOB_READ_WRITE_TOKEN") {
            // `vercel_blob_rw_{store id}_{secret}`. The store id is a field
            // of the token rather than a variable beside it, which is why
            // this deployment's OIDC path needs one and this one does not.
            let store_id =
                token
                    .split('_')
                    .nth(3)
                    .filter(|id| !id.is_empty())
                    .ok_or(Failure::Internal {
                        doing: "reading the blob store's credentials: \
                            BLOB_READ_WRITE_TOKEN is not in the shape a store id can be read from",
                    })?;
            let store_id = store_id.to_owned();
            return Ok(Self { store_id, token });
        }

        Err(Failure::Internal {
            doing: "reading the blob store's credentials: \
                    neither VERCEL_OIDC_TOKEN nor BLOB_READ_WRITE_TOKEN is set",
        })
    }

    /// The credentials this process was deployed with.
    ///
    /// The one line that reads the environment, so that everything deciding
    /// what those variables mean is [`Credentials::resolve`] and can be
    /// asked without a process to set them in.
    pub(crate) fn from_env() -> Result<Self, Failure> {
        Self::resolve(|name| std::env::var(name).ok())
    }
}

/// The Vercel Blob API, as this server talks to it.
///
/// Named for the thing it talks to rather than for what it holds, because
/// [`Blob`] is what a blob is here and a type cannot be both. What a caller
/// asks this for is an operation; what it gets back is a [`Blob`] or
/// nothing.
pub(crate) struct Api {
    /// The API's base URL, without a trailing slash.
    base: String,
    /// The store this client writes to, in the spelling the header wants.
    store_id: String,
    /// The bearer token. Never logged, never put in a [`Failure`].
    token: String,
    /// The wait before a second attempt. A field rather than the constant
    /// read where it is used, so that retrying can be exercised at a
    /// millisecond instead of making the suite wait out a real backoff —
    /// the same reason `Archive` carries its size limit.
    backoff: Duration,
    /// How long one attempt may take. A field for the same reason as
    /// `backoff`, and set on the request rather than on the client, because
    /// the client is built once for the process and this is a policy a test
    /// has to be able to reach.
    timeout: Duration,
}

impl Api {
    /// A client for the store `store_id` at `base`.
    ///
    /// `store_id` is taken as it is provisioned — `store_a-test-store` —
    /// and the prefix comes off here rather than at the call site, so that
    /// the spelling the header wants is this module's business and not
    /// something every caller has to know.
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

    /// The client this deployment runs: the API itself, authorised with
    /// `credentials`.
    ///
    /// Credentials are taken rather than read here, so that the failure a
    /// missing variable produces happens where #21 can act on it — at the
    /// moment the store is built, and not at the first cache write.
    pub(crate) fn live(credentials: Credentials) -> Self {
        Self::at(API_BASE, &credentials.store_id, credentials.token)
    }

    /// The same client, waiting `backoff` before its second attempt.
    #[cfg(test)]
    pub(crate) fn with_backoff(self, backoff: Duration) -> Self {
        Self { backoff, ..self }
    }

    /// The same client, giving one attempt `timeout` and no more.
    #[cfg(test)]
    pub(crate) fn with_timeout(self, timeout: Duration) -> Self {
        Self { timeout, ..self }
    }

    /// Write `body` to `pathname`.
    ///
    /// The pathname is a query parameter and not a path segment, which is
    /// what `@vercel/blob` sends and therefore what the API reads.
    pub(crate) async fn put(&self, pathname: &str, body: Vec<u8>) -> Result<(), Failure> {
        const DOING: &str = "writing to the blob store";

        let mut url = self.url(&format!("{}/", self.base))?;
        url.query_pairs_mut().append_pair("pathname", pathname);

        let request = self
            .request(Method::PUT, url)?
            // Both asked for rather than left to the API's defaults. The
            // path an entry lives at is derived from its contents, so a
            // random suffix would put it somewhere nothing can read it
            // back from; and an entry is written once, so an overwrite
            // would reset the `uploadedAt` that eviction runs in the order of.
            .header("x-add-random-suffix", "0")
            .header("x-allow-overwrite", "0")
            // The API assumes nothing here. `public` because an entry is
            // derived from a published package anyone can already download,
            // and because it is what every store serves without being
            // configured for it.
            .header("x-vercel-blob-access", "public")
            .body(body);

        let response = self.send(request, DOING).await?;
        succeeded(response, DOING)?;
        Ok(())
    }

    /// What the store knows about the blob at `pathname`, if it holds one.
    ///
    /// The parameter is spelled `url` because the API takes either a blob's
    /// URL or its pathname there; this client only ever has a pathname, so
    /// that is what it sends.
    pub(crate) async fn head(&self, pathname: &str) -> Result<Option<Blob>, Failure> {
        let mut url = self.url(&self.base)?;
        url.query_pairs_mut().append_pair("url", pathname);

        const DOING: &str = "asking the blob store for a cached result";

        let response = self.send(self.request(Method::GET, url)?, DOING).await?;

        // A blob that is not there is the ordinary answer to this question
        // and not a refusal to answer it, so it is the one status this
        // operation reads for itself.
        if response.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(None);
        }

        Ok(Some(self.decode(succeeded(response, DOING)?, DOING).await?))
    }

    /// The bytes of the blob at `pathname`, if the store holds one.
    ///
    /// Two requests, because the API has no operation that answers with a
    /// blob's contents: what it answers with is where to download them from.
    /// Both hops are one operation here rather than two at the caller,
    /// because "is it there" and "what is in it" are one question — a cache
    /// hit — and a caller holding half of it would have to decide what half
    /// an entry means. [ADR
    /// 0003](../docs/adr/0003-the-cache-seam-is-a-store.md) says it does not.
    pub(crate) async fn read(&self, pathname: &str) -> Result<Option<Vec<u8>>, Failure> {
        const DOING: &str = "reading a cached result from the blob store";

        let mut url = self.url(&self.base)?;
        url.query_pairs_mut().append_pair("url", pathname);

        let response = self.send(self.request(Method::GET, url)?, DOING).await?;

        // The same answer [`Api::head`] reads for itself, and for the same
        // reason: a blob that is not there is what this question is asked
        // to find out.
        if response.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(None);
        }

        let at: Download = self.decode(succeeded(response, DOING)?, DOING).await?;

        // No credential on the second hop, and none wanted: an entry is
        // derived from a package anyone can already download, which is why
        // [`Api::put`] asks for public access in the first place.
        let contents = client()?
            .request(Method::GET, self.url(&at.download_url)?)
            .timeout(self.timeout);

        let body = succeeded(self.send(contents, DOING).await?, DOING)?
            .bytes()
            .await
            .map_err(|_| Failure::Internal { doing: DOING })?;

        Ok(Some(body.to_vec()))
    }

    /// Every blob under `prefix`, however many pages that takes.
    ///
    /// The walk is this module's and not a caller's. A `list` that handed
    /// back a cursor would make "did you follow it to the end?" a question
    /// at every call site, and the one place it matters — the size the
    /// budget is counted against — is the one place an unfinished walk
    /// looks like a correct small number.
    pub(crate) async fn list(&self, prefix: &str) -> Result<Vec<Blob>, Failure> {
        const DOING: &str = "listing what the blob store holds";

        let mut blobs = Vec::new();
        let mut cursor: Option<String> = None;

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
                Some(next) => cursor = Some(next),
                None => return Ok(blobs),
            }
        }
    }

    /// Delete every blob at `pathnames`.
    ///
    /// Several at once because an entry is several blobs, and they go
    /// together or the entry is half there. The API takes a blob's URL or
    /// its pathname in the same field; this client has pathnames.
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

        // A blob that is already gone is the outcome this call asked for.
        // The API is idempotent here, and not relying on that costs one
        // line: two functions can sweep the same entry at once, and a
        // `list` can name a blob a delete sixty seconds ago has not
        // finished propagating.
        if response.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(());
        }

        succeeded(response, DOING)?;
        Ok(())
    }

    /// Send `request`, trying again while the store's answer says trying
    /// again could help.
    ///
    /// A request that never became a response is always worth another
    /// attempt, because there is no answer in it to read; which answers are
    /// is [`again_might_work`].
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

    /// A response's body, as whatever this client asked the API for.
    ///
    /// `doing` names the operation rather than the cause: an upstream body
    /// is where a credential or a signed URL would ride into a message, so
    /// nothing from the response reaches the [`Failure`] at all.
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

    /// A request to the API, carrying what every request to it must.
    ///
    /// One place rather than four, because a header that is right in three
    /// operations and missing in the fourth is a failure that only the
    /// fourth shows — and the store id in particular is not something the
    /// token carries, so a request without it is a request about no store.
    /// The timeout is here for the same reason: an operation that forgot one
    /// would be the operation that hangs the function.
    ///
    /// `bearer_auth` rather than a `header` call: reqwest marks the value
    /// sensitive, so the credential stays out of the client's own `Debug`
    /// output as well as out of ours.
    fn request(&self, method: Method, url: Url) -> Result<RequestBuilder, Failure> {
        Ok(client()?
            .request(method, url)
            .timeout(self.timeout)
            .header("x-api-version", API_VERSION)
            .header("x-vercel-blob-store-id", &self.store_id)
            .bearer_auth(&self.token))
    }

    /// `text` as a URL.
    ///
    /// The base is a string this process was configured with rather than
    /// anything a caller supplies, so a parse failure here is a deployment
    /// that is wrong rather than a request that is — which is why it takes
    /// the internal channel and carries no cause.
    fn url(&self, text: &str) -> Result<Url, Failure> {
        Url::parse(text).map_err(|_| Failure::Internal {
            doing: "building a blob store request",
        })
    }
}

/// Whether asking the same question again could get a different answer.
///
/// A `5xx` is the store rather than the request: the same bytes sent again
/// may work. So is `429`, which is the one `4xx` that is about the moment
/// instead of about what was asked — and a cache sweep deleting a page of
/// blobs at a time is exactly what provokes one. Every other `4xx` is the
/// store having read the request and refused it, and sending it again is
/// asking a settled question twice.
///
/// How long to wait is this client's own and not the store's `Retry-After`:
/// a cache that waited out the minute somebody else asked for would have
/// spent the whole function on a lookup it can do without.
fn again_might_work(status: reqwest::StatusCode) -> bool {
    status.is_server_error() || status == reqwest::StatusCode::TOO_MANY_REQUESTS
}

/// The response, or the failure its status is.
///
/// `doing` names the operation and nothing from the response reaches the
/// message: a body this server did not write is exactly where a signed URL
/// or a credential would ride into an error, and the caller — #21, which
/// swallows a cache failure rather than showing it — has no use for one.
/// What the cause was belongs in a log, next to the request that produced
/// it (#26).
fn succeeded(response: Response, doing: &'static str) -> Result<Response, Failure> {
    if response.status().is_success() {
        return Ok(response);
    }
    Err(Failure::Internal { doing })
}

/// The one client this module has, built once.
///
/// Separate from [`crate::archive`]'s, and deliberately: that one may follow
/// a redirect only onto a registry's hosts, which is a rule about somebody
/// else's servers and not about this store. Sharing a client would mean one
/// policy serving two sets of reasons.
fn client() -> Result<&'static Client, Failure> {
    static CLIENT: OnceLock<Option<Client>> = OnceLock::new();

    CLIENT
        .get_or_init(|| {
            // The same choice `crate::fetch`'s client makes, and for the
            // same reason: rustls has no default provider, `ring` is the one
            // that needs no C toolchain at build time, and installing it is
            // process-wide with only the first caller winning.
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

    /// One request the stub received, kept as it arrived on the wire.
    struct Received {
        method: Method,
        /// Path and query together, which is what the API's routing reads.
        target: String,
        headers: HeaderMap,
        body: Bytes,
    }

    impl Received {
        /// The value of `name`, or a panic naming the header that was
        /// missing — an absent header and an empty one are different
        /// mistakes, and the one that matters here is absent.
        fn header(&self, name: &str) -> &str {
            self.headers
                .get(name)
                .unwrap_or_else(|| panic!("the request carries no `{name}` header"))
                .to_str()
                .expect("a header this client sets is ASCII")
        }
    }

    /// An HTTP server standing in for the Blob API.
    ///
    /// The client is pointed at it and driven through the real HTTP client,
    /// so what a test asserts is the request this module actually writes —
    /// method, target, headers and body. A stub that answered a canned
    /// response without recording the question could not say any of that,
    /// and the question is the half that has to be right: the API is
    /// private, there is no schema to compile against, and a header spelled
    /// wrong here fails in production and nowhere else.
    struct Stub {
        base: String,
        received: Arc<Mutex<Vec<Received>>>,
    }

    /// What the stub answers, in the order it answers them.
    ///
    /// A queue rather than one canned response, because the operations that
    /// are worth testing are the ones that make more than one request: a
    /// `list` that follows a cursor, and a failure that is retried.
    #[derive(Clone)]
    struct Reply {
        status: StatusCode,
        body: String,
        /// How long the stub holds the request before answering it. A store
        /// that accepts a connection and then says nothing is the case a
        /// status code cannot stand in for.
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

        /// A store that took the request and never answered it.
        fn stalling() -> Self {
            Self {
                stall: Some(Duration::from_secs(30)),
                ..Self::ok("{}")
            }
        }

        /// A refusal, in the shape the API sends one: a status, and a body
        /// carrying the code that says which refusal it is.
        fn refusing(status: StatusCode, code: &str) -> Self {
            Self {
                status,
                body: format!(r#"{{"error":{{"code":"{code}"}}}}"#),
                stall: None,
            }
        }
    }

    /// What the handler shares with the test that started it.
    #[derive(Clone)]
    struct Answers {
        received: Arc<Mutex<Vec<Received>>>,
        replies: Arc<Mutex<VecDeque<Reply>>>,
    }

    impl Stub {
        /// A stub that answers `200 {}` to anything, for the tests that are
        /// about the question rather than the answer.
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

            // A base with a path in it, because the API's own base has one:
            // `put` appends `/?…` to it and `head` appends `?…`, and a
            // host-only base would normalise both to the same target and
            // hide the difference.
            Self {
                base: format!("http://{addr}/api/blob"),
                received,
            }
        }

        fn base(&self) -> String {
            self.base.clone()
        }

        /// Every request the stub was asked, in the order it got them.
        fn requests(&self) -> Vec<Received> {
            std::mem::take(
                &mut *self
                    .received
                    .lock()
                    .expect("the stub's lock is not poisoned"),
            )
        }

        /// The one request the stub was asked, or a panic naming how many it
        /// actually got.
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

    /// The write, as the API is asked for it. The pathname is a *query
    /// parameter* and not a path segment, which is the detail no offline
    /// test would otherwise catch: `PUT /{pathname}` reaches a different
    /// endpoint, succeeds against nothing, and is what the issue this
    /// implements was written against.
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

    /// The three headers every request to this API carries. None of them is
    /// optional and none is discoverable: the version pins the response
    /// shape this client parses, the store id is not encoded in an OIDC
    /// token and so has to be sent beside it, and the bearer is the
    /// credential. A request missing one is answered, which is what makes
    /// this worth asserting rather than assuming.
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

    /// The store id is provisioned as `store_a-test-store` and the
    /// header wants `a-test-store`. `@vercel/blob` strips the prefix on
    /// the way out, so a client that passed the variable through unchanged
    /// would be asking about a store that does not exist — and the answer to
    /// that is a refusal no retry fixes, on every call, from the first
    /// deploy.
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

    /// An entry is written once, at a path derived from its contents, and is
    /// never updated. Both halves of that are asked for here rather than
    /// inherited: a random suffix would make the path unguessable and the
    /// cache unreadable, and an overwrite would reset the `uploadedAt` that
    /// eviction orders by. The API defaults to what this asks for today,
    /// which is exactly why it is asked for — a default is somebody else's
    /// to change.
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

    /// The API has no default for access — `@vercel/blob` refuses to build a
    /// write without it — so every write this client makes has to say. What
    /// it says is `public`, because a cached entry is derived entirely from
    /// a published package that anybody can already download, and because a
    /// public blob is what every provisioned store serves.
    #[tokio::test]
    async fn a_write_declares_the_access_the_api_will_not_assume() {
        let stub = Stub::start().await;
        let api = Api::at(stub.base(), "a-test-store", "a-token");

        api.put("diffs/v1/abc/meta.json", b"{}".to_vec())
            .await
            .expect("the stub answers the way the API does");

        assert_eq!(stub.received().header("x-vercel-blob-access"), "public");
    }

    /// What a `head` is for: a write asks it rather than replacing an entry
    /// that is already there, and a sweep accounts in the same two numbers a
    /// listing answers with. So the two facts a present blob has to come
    /// back with are its size and when it was written — and the request that
    /// asks for them is a `GET` with the pathname in a `url` parameter,
    /// which is the API's spelling and not one to guess at.
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

    /// A miss is the ordinary answer, not a failure. Every cold diff asks
    /// `head` about a blob that is not there, and a client that returned an
    /// error for it would put the cache's commonest case on the path #21
    /// has to recover from — where a real outage also lives, and the two
    /// would be indistinguishable.
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

    /// A page is not the answer. The store holds one blob per file per
    /// cached entry and the budget is a sum over all of them, so a
    /// `list` that stopped at the first page would under-count — silently,
    /// by exactly the blobs it never asked for, and in the safe-looking
    /// direction that lets the cache grow past its ceiling unobserved.
    ///
    /// Two pages, because two is what proves the loop runs at all: the
    /// second request has to carry the cursor the first one answered with,
    /// and the walk has to stop when the store says there is no more.
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

    /// The walk ends even when the store contradicts itself. `hasMore` with
    /// no cursor beside it is a page that says "ask again" and does not say
    /// where — and the obvious reading of it, trusting `hasMore` alone, asks
    /// for the first page again and appends it again, without end. A cache
    /// sweep that never returns is worse than one that under-counts: it
    /// spends the whole function on a `list`, and the diff behind it is
    /// never answered at all.
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

    /// An entry is two blobs that live and die together, and eviction
    /// deletes whole entries — so the call that deletes has to take more
    /// than one path. One request per blob would make a half-deleted entry
    /// an ordinary outcome of a sweep that failed in the middle, and half
    /// an entry is the one thing #21's read path cannot make sense of.
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

    /// Deleting something that is already gone is a success, because by the
    /// time a sweep runs the thing it wants gone may be gone — two
    /// functions can evict the same entry at the same moment, and deletes
    /// take up to a minute to propagate, so a list can name a blob that no
    /// longer exists.
    ///
    /// The API is idempotent here and this client does not rely on that: a
    /// refusal that says the blob is not there is the outcome the caller
    /// asked for, whoever achieved it.
    #[tokio::test]
    async fn deleting_a_blob_that_is_already_gone_is_not_a_failure() {
        let stub = Stub::answering(vec![Reply::refusing(StatusCode::NOT_FOUND, "not_found")]).await;
        let api = Api::at(stub.base(), "a-test-store", "a-token");

        api.delete(&["diffs/v1/evicted-already/meta.json"])
            .await
            .expect("a blob that is already gone is the outcome that was asked for");
    }

    /// A refusal is a failure. It sounds too obvious to assert until you
    /// notice what the alternative looks like from #21: a `put` that
    /// reported success on a `403` would leave the store believing it holds
    /// an entry it does not, and the next `head` would be a miss with no
    /// sign anything went wrong. A cache that fails has to fail out loud
    /// enough to be swallowed deliberately.
    #[tokio::test]
    async fn a_write_the_store_refuses_is_a_failure_and_not_a_silent_success() {
        let stub = Stub::answering(vec![Reply::refusing(StatusCode::FORBIDDEN, "forbidden")]).await;
        let api = Api::at(stub.base(), "a-test-store", "a-token");

        api.put("diffs/v1/abc/meta.json", b"{}".to_vec())
            .await
            .expect_err("a write the store refused is not a write");
    }

    /// The read's half of the same rule, and the half the policy above leans
    /// on. A `404` is a miss the store answered; any other refusal is a read
    /// that failed, and it has to reach the policy as one. That is what
    /// makes a lost read the same Note whichever adapter lost it: a `read`
    /// that turned a `403` into `None` would be a miss that says nothing,
    /// and the suite's staged failure would be louder than production's.
    #[tokio::test]
    async fn a_read_the_store_refuses_is_a_failure_and_not_a_miss() {
        let stub = Stub::answering(vec![Reply::refusing(StatusCode::FORBIDDEN, "forbidden")]).await;
        let api = Api::at(stub.base(), "a-test-store", "a-token");

        api.read("diffs/v1/abc/meta.json")
            .await
            .expect_err("a read the store refused is not a miss");
    }

    /// A store that is briefly unwell is the case worth spending a retry
    /// on: the request was good, nothing about it needs changing, and the
    /// alternative is a diff recomputed from two archive downloads because
    /// one `503` arrived at the wrong moment.
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

    /// `429` is the one `4xx` that asking again can get past, and a cache is
    /// what provokes it: a sweep deletes a page of blobs at a time and a
    /// cold function writes two of them at once. Reading it as a settled
    /// refusal would give up on the one answer that changes by itself.
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

    /// A store that takes the request and then says nothing is the failure a
    /// status code cannot stand in for: there is no answer to read, so
    /// without a deadline this waits until the platform kills the function —
    /// and what the caller gets is a dropped connection rather than a
    /// recomputed diff it could have had in the same time.
    ///
    /// Three attempts each bounded is what makes the whole call bounded,
    /// which is why the deadline is on the request rather than on the
    /// operation: an operation that forgot one is the operation that hangs.
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

    /// The other edge of the same rule, and the one that costs something to
    /// get wrong. A `404` is the store having read the request and answered
    /// it; asking again is asking a settled question twice, and doing it
    /// under a retry budget turns every cold cache — the commonest thing
    /// this client does — into three round trips and two sleeps before the
    /// diff can even start.
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

    /// The claim `tests/errors.rs` left for this issue to make. It asserted
    /// redaction against a token-shaped string because this client did not
    /// exist; this is the same claim against a failure built inside it.
    ///
    /// The token is put in both places a failure could pick one up — the
    /// request this client sends, and the body the store refuses with — and
    /// then every rendering of the failure is read for it: the `Debug` a log
    /// line would carry, and the text that leaves on whichever channel
    /// `respond` puts it on.
    ///
    /// The defence being checked is structural rather than a filter. A
    /// failure here is built from a fixed phrase this module chose, so there
    /// is nothing for a credential to arrive in. What this test catches is
    /// the day someone adds a variant that carries the store's own words.
    #[tokio::test]
    async fn no_failure_this_client_builds_carries_the_token() {
        // Assembled rather than written out, for the reason
        // `tests/errors.rs` gives at length: a literal of this shape is
        // indistinguishable from a real credential to a secret scanner, and
        // a check that cries wolf is one people learn to click past.
        //
        // Do not "tidy" this back into one literal.
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

    // -----------------------------------------------------------------
    // Credentials
    // -----------------------------------------------------------------
    //
    // The second seam, and it is a function over a lookup rather than over
    // the process environment on purpose: `std::env::set_var` is global to
    // the test binary and unsound beside threads that read it, so tests
    // that set variables cannot run next to each other or next to anything
    // else. A lookup makes each of these a pure question with a written-down
    // answer.

    /// An environment, as `resolve` reads one.
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

    /// What this project is actually provisioned with. There is no
    /// read-write token on it at all: the store id is an environment
    /// variable and the credential is an OIDC token the platform mints and
    /// refreshes, which is why the two are separate here rather than one
    /// string the store id can be read out of.
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

    /// The fallback, for a deployment given a read-write token instead.
    /// There is no store id variable in that case, because the token is the
    /// store id: it is the fourth underscore-separated field of it. This is
    /// the only place this client reads a credential for anything but its
    /// value, and it is why the field exists at all.
    ///
    /// Assembled rather than written out, for the reason given above.
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

    /// A misconfigured deployment has to say which variable to set, and say
    /// it where it can be read: resolving here rather than at the first
    /// cache write is the difference between a deploy that fails and a
    /// deploy that looks healthy and quietly caches nothing for a week,
    /// because a cache failure is swallowed by design (#21).
    ///
    /// Three ways to be misconfigured, and they are three because the
    /// remedies differ: add the store id, set a credential at all, or
    /// replace a token that is not one.
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
            // Matched rather than `expect_err`ed, because that would need
            // `Credentials` to be `Debug` — and a derived `Debug` on a type
            // holding a bearer token is the leak this module already spends
            // a test on not having.
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

    // -----------------------------------------------------------------
    // Against the real store
    // -----------------------------------------------------------------
    //
    // Three of the facts this client is built on are not guessable, and a
    // stub cannot disconfirm any of them: that a write's pathname is a query
    // parameter rather than a path segment, that the store id is sent without
    // the `store_` it is provisioned with, and that `access` has no default.
    // A stub written from the same reading as the client agrees with the
    // client whether or not the reading was right, so the only thing that can
    // settle them is the store.
    //
    // So this reaches it, and is `#[ignore]`d for the same reason every test
    // in `tests/networked.rs` is: `cargo test` stays offline and
    // deterministic, and these are run deliberately.
    //
    // ```text
    // cargo test --lib -- --ignored store::blob
    // ```

    /// The four operations against the store this project is provisioned
    /// with, in the order the cache uses them: write an entry's blob, ask
    /// whether it is there, find it under its prefix, delete it — and delete
    /// it again, because eviction has to survive two sweeps reaching the same
    /// blob and only the store can say that it does.
    ///
    /// One test rather than five. The operations are not independent: there
    /// is nothing to `head` that was not first written, and a `delete` on its
    /// own would be deleting whatever the last run left behind.
    ///
    /// What it does not prove is the cursor. Filling more than one page means
    /// writing a thousand blobs into a store with a 256 MB budget over it,
    /// which is a worse thing to leave behind than the walk is to assert
    /// against a stub — so the stub keeps that one and this keeps the shape
    /// of the request.
    #[tokio::test]
    #[ignore = "networked: writes to this project's Vercel Blob store"]
    async fn the_real_store_answers_the_four_requests_this_client_makes() {
        let credentials = Credentials::from_env()
            .expect("VERCEL_OIDC_TOKEN with BLOB_STORE_ID, or BLOB_READ_WRITE_TOKEN");
        let api = Api::live(credentials);

        // Unique per run: a write refuses to overwrite, and two runs at once
        // must not delete each other's blob out from under the `head`.
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
