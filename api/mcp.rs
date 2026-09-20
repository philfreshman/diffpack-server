//! The one function this repo deploys.
//!
//! Vercel's Rust runtime wants a `[[bin]]` per handler under `api/`, and
//! `vercel.json` (#8) rewrites every path to this one. So routing happens
//! inside the function rather than in the platform, and this file stays a
//! thin entry point: it matches a path, asks the library what to say, and
//! puts the answer on a response. Anything with a decision in it belongs in
//! `src/`, where a test can reach it without a runtime.
//!
//! Today that is `/health` alone. #6 replaces the match below with an axum
//! `Router` carrying `rmcp`'s `StreamableHttpService` at `/mcp`.

use diffpack_server::health;
use vercel_runtime::{run, service_fn, Error, Request, Response, ResponseBody};

#[tokio::main]
async fn main() -> Result<(), Error> {
    run(service_fn(handler)).await
}

async fn handler(req: Request) -> Result<Response<ResponseBody>, Error> {
    match req.uri().path() {
        "/health" => json(200, health::body()),
        _ => json(404, serde_json::json!({ "error": "not found" })),
    }
}

fn json(status: u16, body: serde_json::Value) -> Result<Response<ResponseBody>, Error> {
    Ok(Response::builder()
        .status(status)
        .header("content-type", "application/json")
        .body(body.to_string().into())?)
}
