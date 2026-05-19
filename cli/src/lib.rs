//! `skill-pool-cli` — shared library for the `skill-pool` binary and the
//! long-lived `skill-pool-capturer` daemon.
//!
//! The crate lives as a library so two `[[bin]]` targets can share the
//! same modules (capture pipeline, scorer, anthropic client, …) without
//! duplicating compilation. End-users still drive everything through
//! the binaries; this lib has no semver promise.

pub mod anthropic;
pub mod banner;
pub mod capturer;
pub mod client;
pub mod cmd;
pub mod config;
pub mod detect;
pub mod install;
pub mod manifest;
pub mod notify;
pub mod scorer;
pub mod secret_scan;
