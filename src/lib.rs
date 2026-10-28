//! Airlock: Universal multi-agent workflow coordinator and task management hub.

pub mod config;
pub mod core;

/// Airlock crate version
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
