//! Airlock: local-first multi-agent workflow coordinator and task management hub.

pub mod app;
pub mod briefing;
pub mod cli;
pub mod config;
pub mod core;
pub mod doctor;
pub mod git;
pub mod hub;
pub mod init;
pub mod layout;
pub mod mcp;
pub mod overlap;
pub mod setup;
pub mod snapshot;
pub mod store;
pub mod sync;
pub mod views;
pub mod web;

/// Airlock crate version
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
