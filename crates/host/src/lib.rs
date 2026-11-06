//! `shotd` composition root: one explicit root, one writer, real services, secure listener.
//!
//! This crate owns the S01 §6.3 bootstrap order and the §6.5 shutdown order. It never
//! creates roots, never adopts a foreign tree and never resolves a path from CWD, PATH
//! or an environment variable: the only accepted root is the explicit `--root` argument.
pub mod access;
mod args;
mod bootstrap;
mod capture;
pub mod processing;
mod processing_execution;
mod runtime_config;
pub use runtime_config::read_backend_secret;
mod services;

pub use args::{parse_bootstrap_args, ArgError, BootstrapArgs, BootstrapMode};
pub use bootstrap::{
    bootstrap, bootstrap_from_installation, bootstrap_with_processing, emit_pre_root_failure,
    BootstrapOptions, BootstrappedHost, CaptureOptions,
};
pub use bootstrap::{HostError, ShutdownOutcome, DEFAULT_DRAIN, SHUTDOWN_BUDGET};
pub use services::FoundationServices;

/// Process exit codes of `shotd`; the same contract the CLI uses for host/runtime errors.
pub const EXIT_OK: u8 = 0;
pub const EXIT_USAGE: u8 = 2;
pub const EXIT_HOST: u8 = 4;
pub const EXIT_POLICY: u8 = 6;
pub const EXIT_STORAGE: u8 = 7;
pub const EXIT_WAIT: u8 = 8;
