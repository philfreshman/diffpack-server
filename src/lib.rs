//! The diffpack MCP server.
//!
//! `api/mcp.rs` is the deployed entry point and stays thin; everything with a
//! decision in it lives here, where a test can reach it without a runtime.

pub mod archive;
pub mod cache_key;
pub mod catalogue;
pub mod engine;
pub mod error;
pub mod handle;
pub mod health;
// Private on purpose: the client is `archive`'s and `catalogue`'s to use and
// nobody else's, and a module a tool cannot name is a rule the compiler keeps
// rather than a rule a script notices.
mod http;
pub mod mcp;
pub mod page;
pub mod registry;
pub mod router;
pub mod tools;
