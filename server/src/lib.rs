//! skill-pool registry library.
//!
//! Exposes the building blocks (`Config`, `AppState`, `routes::router`, `admin`)
//! so integration tests can compose them differently from the binary's main.
//! The binary (`src/main.rs`) is a thin clap-driven wrapper over these.

pub mod admin;
pub mod audit;
pub mod auth;
pub mod bundle;
pub mod cache;
pub mod config;
pub mod css_sanitize;
pub mod email_branding;
pub mod embedding;
pub mod error;
pub mod git_sync;
pub mod logo_sanitize;
pub mod metrics;
pub mod notify;
pub mod plugin;
pub mod queue;
pub mod rate_limit;
pub mod routes;
pub mod state;
pub mod storage;
pub mod telemetry;
pub mod tenant;
pub mod tracing_setup;
pub mod worker;
