//! The one route that exists before the protocol does.
//!
//! `/health` is what #8 deploys to prove the Vercel pipeline end to end —
//! build, rewrite, cold start, response — and it outlives that job: it is the
//! one check that needs no MCP client, so it is what a monitor watches and
//! what someone curls when `/mcp` is misbehaving. It names the build that
//! answered, so a deploy that silently served the previous binary is visible
//! rather than inferred.

use serde_json::{json, Value};

/// The body `GET /health` answers with.
pub fn body() -> Value {
    json!({
        "status": "ok",
        "service": "diffpack-server",
        "version": env!("CARGO_PKG_VERSION"),
    })
}
