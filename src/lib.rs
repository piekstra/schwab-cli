//! `schw` — piekstra-family CLI for Charles Schwab's Trader API.
//!
//! Read-only. See `main.rs` for the command surface and `docs/api.md` for what
//! the API does and does not expose.

pub mod client;
pub mod commands;
pub mod config;
pub mod credential;
pub mod oauth;
pub mod parse;
pub mod urlenc;
