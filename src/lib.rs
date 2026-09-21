//! The diffpack MCP server.
//!
//! `api/mcp.rs` is the deployed entry point and stays thin; everything with a
//! decision in it lives here, where a test can reach it without a runtime.

pub mod archive;
pub mod cache_key;
pub mod catalogue;
pub mod engine;
pub mod error;
pub mod fetch;
pub mod handle;
pub mod health;
pub mod log;
pub mod mcp;
pub mod page;
pub mod registry;
pub mod router;
pub mod tools;
