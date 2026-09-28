//! `shot` entry point: explicit trusted configuration, one JSON to stdout, stable exits.
use sm_application::credentials::SecretStore;
use sm_cli::{run, CliDeps, ProcessLauncher, EXIT_HOST};
use sm_credentials::WindowsSecretStore;
use sm_isolation::Environment;
use std::{
    io::{stderr, stdout},
    process::ExitCode,
    sync::Arc,
};

fn main() -> ExitCode {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(_) => return ExitCode::from(EXIT_HOST),
    };
    let store: Arc<dyn SecretStore + Send + Sync> =
        Arc::new(WindowsSecretStore::new(Environment::Test));
    let deps = CliDeps {
        store,
        launcher: Arc::new(ProcessLauncher),
    };
    let code = runtime.block_on(run(&argv, &deps, &mut stdout(), &mut stderr()));
    ExitCode::from(code)
}
