//! Executing a parsed `.run` target: property resolution, shell dispatch and
//! the statement walker. Kept out of `runfile-lang` so the LSP can parse and
//! evaluate without linking process-spawning.

pub mod env;
pub mod exec;
pub mod props;
pub mod run;
pub mod shell;

pub use props::Props;
pub use run::{RunError, Runner, run_target};

#[cfg(test)]
mod tests;
