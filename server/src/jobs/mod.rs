//! Background job handlers.
//!
//! Each module in this directory implements `queue::Job` + `worker::JobHandler`
//! for one job kind. Handlers are registered with the `Worker` at startup in
//! `main.rs`.

pub mod plugin_mirror;
