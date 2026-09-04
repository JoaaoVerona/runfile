//! Machine-local state: OS credential-store access and the prepare gate's
//! record. There is no user settings file and no `run :config` -- the one
//! machine-wide directory is `$HOME/.runfiles/` (or `runfiles`/`Runfiles`),
//! at a fixed set of names.

pub mod keyring_keys;
pub mod keyring_store;
mod paths;
mod prepare_state;
#[cfg(target_os = "linux")]
mod secret_service_store;

pub use paths::*;
pub use prepare_state::*;

#[cfg(test)]
mod tests;
