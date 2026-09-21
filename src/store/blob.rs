//! The Vercel Blob client.
//!
//! Four operations, because four is what the cache needs: write a blob, ask
//! whether one is there, list what is under a prefix, delete several at
//! once. It is not a general client for the service and is not meant to
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

// Every operation below is written for `DiffStore` (#21) to call, and until
// that exists nothing in the crate calls any of them. The client is private
// to this module by design — ADR 0003 — so there is no public surface for
// the compiler to count as a use either. This allow comes off with #21.
#![allow(dead_code)]

use std::sync::OnceLock;

use reqwest::{Client, Method, RequestBuilder, Url};

use crate::error::Failure;

/// The API revision this client is written against.
///
/// It decides the shape of what comes back, so it is not a number to bump
/// idly: the parsing below is written for this version and for no other.
const API_VERSION: &str = "12";

/// The Vercel Blob API, as this server talks to it.
pub(crate) struct Blob {
    /// The API's base URL, without a trailing slash.
    base: String,
    /// The store this client writes to, in the spelling the header wants.
    store_id: String,
    /// The bearer token. Never logged, never put in a [`Failure`].
    token: String,
}

impl Blob {
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
        }
    }

    /// Write `body` to `pathname`.
    ///
    /// The pathname is a query parameter and not a path segment, which is
    /// what `@vercel/blob` sends and therefore what the API reads.
    pub(crate) async fn put(&self, pathname: &str, body: Vec<u8>) -> Result<(), Failure> {
        let mut url = self.url(&format!("{}/", self.base))?;
        url.query_pairs_mut().append_pair("pathname", pathname);

        self.request(Method::PUT, url)?
            // Both asked for rather than left to the API's defaults. The
            // path an entry lives at is derived from its contents, so a
            // random suffix would put it somewhere nothing can read it
            // back from; and an entry is written once, so an overwrite
            // would reset the `uploadedAt` that #22 evicts in the order of.
            .header("x-add-random-suffix", "0")
            .header("x-allow-overwrite", "0")
            // The API assumes nothing here. `public` because an entry is
            // derived from a published package anyone can already download,
            // and because it is what every store serves without being
            // configured for it.
            .header("x-vercel-blob-access", "public")
            .body(body)
            .send()
            .await
            .map_err(|_| Failure::Internal {
                doing: "writing to the blob store",
            })?;
        Ok(())
    }

    /// A request to the API, carrying what every request to it must.
    ///
    /// One place rather than four, because a header that is right in three
    /// operations and missing in the fourth is a failure that only the
    /// fourth shows — and the store id in particular is not something the
    /// token carries, so a request without it is a request about no store.
    ///
    /// `bearer_auth` rather than a `header` call: reqwest marks the value
    /// sensitive, so the credential stays out of the client's own `Debug`
    /// output as well as out of ours.
    fn request(&self, method: Method, url: Url) -> Result<RequestBuilder, Failure> {
        Ok(client()?
            .request(method, url)
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
            // The same choice `crate::archive`'s client makes, and for the
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

    use std::sync::{Arc, Mutex};

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

    impl Stub {
        async fn start() -> Self {
            let received: Arc<Mutex<Vec<Received>>> = Arc::default();
            let app = Router::new()
                .fallback(answer)
                .with_state(Arc::clone(&received));

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

    async fn answer(
        State(received): State<Arc<Mutex<Vec<Received>>>>,
        request: Request,
    ) -> Response {
        let (parts, body) = request.into_parts();
        let body = axum::body::to_bytes(body, usize::MAX)
            .await
            .expect("a test request body is small enough to buffer");

        received
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

        Response::builder()
            .status(StatusCode::OK)
            .body(Body::from("{}"))
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
        let blob = Blob::at(stub.base(), "a-test-store", "a-token");

        blob.put("diffs/v1/abc/meta.json", b"{\"cached\":true}".to_vec())
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
        let blob = Blob::at(stub.base(), "a-test-store", "a-token");

        blob.put("diffs/v1/abc/meta.json", b"{}".to_vec())
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
        let blob = Blob::at(stub.base(), "store_a-test-store", "a-token");

        blob.put("diffs/v1/abc/meta.json", b"{}".to_vec())
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
        let blob = Blob::at(stub.base(), "a-test-store", "a-token");

        blob.put("diffs/v1/abc/meta.json", b"{}".to_vec())
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
        let blob = Blob::at(stub.base(), "a-test-store", "a-token");

        blob.put("diffs/v1/abc/meta.json", b"{}".to_vec())
            .await
            .expect("the stub answers the way the API does");

        assert_eq!(stub.received().header("x-vercel-blob-access"), "public");
    }
}
