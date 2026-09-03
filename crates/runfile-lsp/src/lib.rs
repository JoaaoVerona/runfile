//! A language server for `.run` files.
//!
//! The analysis is the real parser, so an editor and the runner can never
//! disagree about whether a file is valid.

pub mod analysis;
pub mod rpc;
pub mod server;
pub mod shell;
