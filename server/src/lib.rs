//! skill-pool registry library.
//!
//! Exposes the building blocks (`Config`, `AppState`, `routes::router`, `admin`)
//! so integration tests can compose them differently from the binary's main.
//! The binary (`src/main.rs`) is a thin clap-driven wrapper over these.

pub mod admin;
pub mod audit;
pub mod auth;
pub mod bundle;
pub mod config;
pub mod embedding;
pub mod error;
pub mod metrics;
pub mod notify;
pub mod routes;
pub mod state;
pub mod storage;
pub mod tenant;
