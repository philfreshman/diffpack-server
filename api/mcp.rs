//! The one function this repo deploys.
//!
//! Vercel's Rust runtime wants a `[[bin]]` per handler under `api/`, and
//! `vercel.json` (#8) rewrites every path to this one. So routing happens
//! inside the function rather than in the platform — and it happens in
//! `src/router.rs`, not here, because a test can reach a `Router` and cannot
//! reach a process.
//!
//! What is left is the adapter: `VercelLayer` turns the runtime's request and
//! response types into axum's and back. Everything with a decision in it is
//! on the other side of it.

use tower::Layer;
use vercel_runtime::axum::VercelLayer;
use vercel_runtime::{run, Error};

#[tokio::main]
async fn main() -> Result<(), Error> {
    run(VercelLayer.layer(diffpack_server::router::router())).await
}
