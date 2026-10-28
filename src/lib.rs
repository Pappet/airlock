//! Airlock: local-first multi-agent workflow coordinator and task management hub.

pub mod app;
pub mod cli;
pub mod config;
pub mod core;
pub mod hub;
pub mod mcp;
pub mod snapshot;
pub mod store;
pub mod sync;
pub mod views;

/// Airlock crate version
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
