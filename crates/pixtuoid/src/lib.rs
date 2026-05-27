//! Public surface for the pixtuoid binary's internals — exposed so
//! examples and integration tests can import them. The `main.rs` binary is
//! the primary entry point.

pub mod cli;
pub mod config;
pub mod init_pack;
pub mod install;
pub mod runtime;
pub mod tui;
pub mod validate;
