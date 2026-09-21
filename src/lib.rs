//! The diffpack MCP server.
//!
//! `api/mcp.rs` is the deployed entry point and stays thin; everything with a
//! decision in it lives here, where a test can reach it without a runtime.

pub mod cache_key;
pub mod engine;
pub mod error;
pub mod health;
pub mod mcp;
pub mod router;
pub mod tools;
