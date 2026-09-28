//! `shot` client surface: one JSON to stdout, safe diagnostics to stderr, stable exit codes.
//!
//! The CLI never opens SQLite, never selects a second host implementation and never
//! resolves a root or a host image from CWD, PATH or the environment: every input comes
//! from the explicit trusted preview configuration named by `--config`.
mod args;
mod config;
mod exec;
mod launcher;
mod output;

pub use args::{parse_args, ArgError, CliArgs, Command};
pub use config::{
    load_config, resolve_target, ConfigError, PreviewConfig, ResolvedTarget, TargetError,
};
pub use exec::{
    client_failure, config_message, connect_session, doctor_exit_code, exit_for_code, run,
    session_failure, target_failure, CliDeps, ClientFailure,
};
pub use launcher::{HostLauncher, LaunchError, LaunchedHost, ProcessLauncher};

/// Exit codes inherited from master/S01 §10.3.
pub const EXIT_OK: u8 = 0;
pub const EXIT_USAGE: u8 = 2;
pub const EXIT_HOST: u8 = 4;
pub const EXIT_POLICY: u8 = 6;
pub const EXIT_STORAGE: u8 = 7;
pub const EXIT_WAIT: u8 = 8;
/// Bounded discovery window for an explicitly authorized host autostart.
pub const START_DEADLINE: std::time::Duration = std::time::Duration::from_secs(5);
pub const RETRY_INTERVAL: std::time::Duration = std::time::Duration::from_millis(100);
