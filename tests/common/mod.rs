//! One MCP client, for every suite that drives one.
//!
//! The real test surface of this crate is one request to the router over a
//! [`Ctx`]: a tool suite asks `tools/call`, the collection suite asks
//! `tools/list`, the resource suite asks `resources/read`, and each of them
//! reads a JSON-RPC envelope back. Everything around that request is the same
//! every time — the URI, the `Accept` pair, the SEP-2243 headers, the `_meta`
//! block, the revision the client claims to speak — and none of it is what
//! any suite is asking about.
//!
//! It used to be written out per suite, which is how fourteen files came to
//! hold their own copy of a protocol revision and three different spellings
//! of one header rule. Three spellings is the part that mattered: a suite
//! that gets a header wrong is a suite testing a client this server will
//! never see, and it passes just the same. So the rules live here once, and a
//! suite starts at its first assertion rather than at forty-five lines of
//! harness.
//!
//! `Cargo.toml` declares no `[[test]]` sections, so a directory under
//! `tests/` is not a test target: every suite picks this up with
//! `mod common;` and nothing has to be registered anywhere.
//!
//! # What is here and what is not
//!
//! Here: the envelope, the headers, the revision, and the router built over a
//! `Ctx` the caller supplies. Not here: what any of it means. A client is a
//! way to ask, and every assertion about the answer stays in the suite whose
//! question it is — otherwise this module becomes the second place a rule
//! about tools lives, which is the shape the tool collection itself was
//! written to avoid.
//!
//! # Why the context is a parameter and never a default
//!
//! [`Client::fixture`] is the common case and it is still spelled out at each
//! call site, because a client that quietly built a live context would reach
//! a registry from CI the first time a suite added a call that touched a
//! seam. That is the bug #64 fixed, and the version of it this module could
//! reintroduce for everybody at once. There is no constructor here that does
//! not say which world it is in.

// This module is compiled into every suite that names it, so anything only
// some of them use is dead code in the rest — and `cargo clippy --all-targets
// -- -D warnings` turns that into a failed build. The alternative is one
// `#[allow]` per item, which is the same allowance written eleven times.
#![allow(dead_code)]

use std::sync::Arc;

use axum::body::Body;
use axum::http::{HeaderMap, Request, StatusCode};
use axum::Router;
use diffpack_server::mcp::Diffpack;
use diffpack_server::router;
use diffpack_server::tools::Ctx;
use http_body_util::BodyExt;
use serde_json::{json, Value};
use tower::ServiceExt;

/// The revision this server is written against: no protocol-level sessions,
/// no `initialize` handshake, `server/discover` mandatory.
///
/// One declaration, so moving to the next revision is one edit rather than
/// fourteen that have to agree.
pub const CURRENT: &str = "2026-07-28";

/// The previous revision, still spoken by clients that have not caught up.
/// One endpoint has to serve both, so the suites that matter run against this
/// one too — see [`Client::speaking`].
pub const PREVIOUS: &str = "2025-11-25";

/// The fixture sets a suite is served, instead of the registries.
///
/// The root rather than one seam's directory inside it: [`Ctx::fixture`]
/// gives each seam its own, so a suite naming the root cannot wire one of
/// them to another's set.
pub const FIXTURES: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/fixtures");

/// How a [`Client`] gets a router to drive.
///
/// A factory rather than a router, because `oneshot` consumes the service it
/// is given and a suite sends more than one request.
type Build = Arc<dyn Fn() -> Router + Send + Sync>;

/// A conforming MCP client, pointed at a server the caller chose.
///
/// Cheap to build and cheap to clone: what it holds is a way to make a router
/// and the revision it claims to speak.
#[derive(Clone)]
pub struct Client {
    build: Build,
    speaks: &'static str,
}

impl Client {
    /// A client over a server whose every seam reads the checked-in fixture
    /// sets.
    ///
    /// What almost every suite wants. A context is built per request, the way
    /// the factory in `src/router.rs` builds one in production, so nothing a
    /// suite does in one call is carried into the next by accident.
    pub fn fixture() -> Self {
        Self::building(|| Ctx::fixture(FIXTURES))
    }

    /// A client over a server holding `ctx`, cloned into each request.
    ///
    /// For a suite whose subject is something the context remembers — a cache
    /// with blobs in it, a seam that answers differently the second time. A
    /// `Ctx` shares its seams through an `Arc`, so cloning it is what
    /// production does and building a fresh one would not be.
    pub fn over(ctx: Ctx) -> Self {
        Self::building(move || ctx.clone())
    }

    /// A client over a server whose context `build` makes, once per request.
    ///
    /// The seam `tests/log.rs` needs: the phases a log line reports are one
    /// call's, and a context shared between two calls would put the first
    /// one's fetches in the second one's window.
    pub fn building(build: impl Fn() -> Ctx + Send + Sync + 'static) -> Self {
        let build = Arc::new(build);
        Self::routed(move || {
            let build = Arc::clone(&build);
            router::router_with(move || Ok(Diffpack::with_ctx(build())), Vec::new())
        })
    }

    /// A client over a router the suite built itself.
    ///
    /// For the two questions a context cannot express: which browser origins
    /// are allowed, and what happens when the handler under the transport is
    /// not this crate's — a handler that panics, or one that answers a method
    /// this crate has not implemented.
    pub fn routed(build: impl Fn() -> Router + Send + Sync + 'static) -> Self {
        Self {
            build: Arc::new(build),
            speaks: CURRENT,
        }
    }

    /// The same client, speaking `version` instead of [`CURRENT`].
    ///
    /// What a revision changes is everything SEP-2243 and SEP-1319 added, so
    /// it is one switch rather than a set of them: on [`PREVIOUS`] the
    /// request carries none of the headers and none of the `_meta`, which is
    /// exactly what a client that has not caught up sends.
    pub fn speaking(self, version: &'static str) -> Self {
        Self {
            speaks: version,
            ..self
        }
    }

    /// Call `tool` with `arguments`, answering with the `result`.
    ///
    /// Panics with the JSON-RPC error instead of returning it, so a suite
    /// that built a call wrongly is told what the server objected to rather
    /// than reading `null` out of a missing result. A tool that *failed* is
    /// not that: every answer here is an HTTP `200` whatever is in the body,
    /// and a tool error is in the result.
    pub async fn call(&self, tool: &str, arguments: Value) -> Value {
        self.respond(json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "tools/call",
            "params": { "name": tool, "arguments": arguments },
        }))
        .await
        .result()
    }

    /// Read the resource at `uri`, answering with the `result`.
    pub async fn read(&self, uri: &str) -> Value {
        self.respond(json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "resources/read",
            "params": { "uri": uri },
        }))
        .await
        .result()
    }

    /// Every tool, as `tools/list` returns it.
    ///
    /// The collection rather than one member of it, because a rule every tool
    /// is held to is asserted over what the server actually offers — a test
    /// that named its tools would only ever hold the ones written before it.
    pub async fn tools(&self) -> Vec<Value> {
        let answer = self
            .post(json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "tools/list",
                "params": {},
            }))
            .await;

        answer["result"]["tools"]
            .as_array()
            .unwrap_or_else(|| panic!("tools/list should answer with an array, got {answer}"))
            .clone()
    }

    /// The listed definition of `name`, or a panic naming what was listed.
    pub async fn listed(&self, name: &str) -> Value {
        let tools = self.tools().await;

        tools
            .iter()
            .find(|tool| tool["name"] == name)
            .unwrap_or_else(|| {
                let names: Vec<&str> = tools.iter().filter_map(|t| t["name"].as_str()).collect();
                panic!("`{name}` should be listed, got {names:?}")
            })
            .clone()
    }

    /// Send `body` as a JSON-RPC request, answering with the envelope.
    ///
    /// For the questions `call` cannot ask: a method that does not exist,
    /// arguments that do not validate, a result and a protocol error being
    /// told apart.
    pub async fn post(&self, body: Value) -> Value {
        self.respond(body).await.json()
    }

    /// The same, with the whole of what the client received.
    ///
    /// The status and the bytes as well as the parsed body, because not every
    /// answer is JSON — a `405` is plain text — and because what the response
    /// ceiling bounds is the frame this server wrote, and two frames that
    /// parse alike can still differ in size.
    pub async fn respond(&self, body: Value) -> Answer {
        let body = self.envelope(body);
        self.send(self.request(&body)).await
    }

    /// Drive `request` through a router this client builds.
    ///
    /// The seam for a request a suite assembled itself: a `GET`, a request
    /// with an `Origin`, one missing a header on purpose. Everything else
    /// here arrives through [`Client::request`], which cannot build one of
    /// those.
    pub async fn send(&self, request: Request<Body>) -> Answer {
        let response = (self.build)()
            .oneshot(request)
            .await
            .expect("the router answers every request");

        let status = response.status();
        let headers = response.headers().clone();
        let body = response
            .into_body()
            .collect()
            .await
            .expect("the body should read")
            .to_bytes()
            .to_vec();

        Answer {
            status,
            headers,
            body,
        }
    }

    /// `body`, with the per-request `_meta` a client of this revision
    /// attaches.
    ///
    /// Dropping the `initialize` handshake had to put what it carried
    /// somewhere, and with no session to have agreed it in earlier, that
    /// somewhere is every request. A client on [`PREVIOUS`] attaches none of
    /// it, which is the whole of why this is a method and not a constant.
    fn envelope(&self, body: Value) -> Value {
        let mut body = body;
        if self.speaks >= CURRENT {
            body["params"]["_meta"] = json!({
                "io.modelcontextprotocol/protocolVersion": self.speaks,
                "io.modelcontextprotocol/clientCapabilities": {},
            });
        }
        body
    }

    /// `body`, as an HTTP request a conforming client of this revision sends.
    ///
    /// Everything below is the client's obligation under the transport rather
    /// than this server's leniency, which is why the builder always meets it:
    /// a request that skipped any of it would be testing how this server
    /// treats a broken client rather than whether it serves a working one.
    ///
    /// * `Accept` names both `application/json` and `text/event-stream`, and
    ///   `Content-Type` is JSON.
    /// * From `2026-07-28`, SEP-2243 repeats what the request is about in
    ///   headers, so an intermediary can route and cache without parsing the
    ///   body: `Mcp-Method` on every request, and `Mcp-Name` on the ones that
    ///   name something — the tool for `tools/call`, the URI for
    ///   `resources/read`. The transport refuses a request that omits one
    ///   with `-32020`.
    ///
    /// The one spelling of that rule. It used to have three — conditional,
    /// unconditional, and a `match` — and the two that were not the
    /// conditional one were each a suite away from testing a client nobody
    /// writes.
    pub fn request(&self, body: &Value) -> Request<Body> {
        let mut request = Request::builder()
            .method("POST")
            .uri("/mcp")
            .header("host", "mcp.diffpack.io")
            .header("accept", "application/json, text/event-stream")
            .header("content-type", "application/json")
            .header("mcp-protocol-version", self.speaks);

        // ISO-8601 dates sort lexicographically, so a string comparison is
        // the "this revision and later" the spec's own wording means.
        if self.speaks >= CURRENT {
            let method = body["method"].as_str().expect("a call names a method");
            request = request.header("mcp-method", method);

            for named in ["name", "uri"] {
                if let Some(names) = body["params"][named].as_str() {
                    request = request.header("mcp-name", names);
                }
            }
        }

        request
            .body(Body::from(body.to_string()))
            .expect("the request should build")
    }
}

/// What a client got back.
///
/// The body is left as bytes: not every answer is JSON, and a suite that
/// assumed otherwise would report a parse failure instead of the status it
/// meant to check.
pub struct Answer {
    pub status: StatusCode,
    headers: HeaderMap,
    body: Vec<u8>,
}

impl Answer {
    /// The body, parsed — or a panic showing what came back instead, with the
    /// status, because a non-JSON body is nearly always a status worth
    /// reading.
    pub fn json(&self) -> Value {
        serde_json::from_slice(&self.body).unwrap_or_else(|e| {
            panic!(
                "expected a JSON body ({}), got {e}: {}",
                self.status,
                String::from_utf8_lossy(&self.body)
            )
        })
    }

    /// The `result` of a successful JSON-RPC response, or a panic naming the
    /// error — so a failing test says what the server objected to rather than
    /// `None`.
    pub fn result(&self) -> Value {
        let body = self.json();
        if let Some(error) = body.get("error") {
            panic!("expected a result, got JSON-RPC error {error}");
        }
        body["result"].clone()
    }

    /// The frame as the client received it.
    ///
    /// What the response ceiling is counted in. The bytes rather than the
    /// structure, because what Vercel refuses is what this server wrote.
    pub fn frame(&self) -> String {
        String::from_utf8_lossy(&self.body).into_owned()
    }

    /// The bytes, unread.
    pub fn bytes(&self) -> &[u8] {
        &self.body
    }

    /// One response header, if it is there and is text.
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers.get(name).and_then(|value| value.to_str().ok())
    }
}
