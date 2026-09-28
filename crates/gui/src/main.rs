//! `screenmemory-gui` entry point.
//!
//! Two modes, both requiring an explicit absolute `--config`:
//! * `--self-check` — headless connect + status + doctor, one JSON on stdout, CLI exits;
//! * otherwise — the diagnostic window over the same client, built from the checked-in
//!   frontend assets (no dev server, no package manager, no network).
use sm_application::credentials::SecretStore;
use sm_cli::{ProcessLauncher, EXIT_HOST, EXIT_USAGE};
use sm_credentials::WindowsSecretStore;
use sm_gui::{
    load_window_config, parse_gui_args, run_self_check, run_window, GuiArgError, GuiSession,
    ShellState, SystemReportOpener,
};
use sm_isolation::Environment;
use std::{
    io::{stderr, stdout},
    process::ExitCode,
    sync::Arc,
};

fn main() -> ExitCode {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    let store: Arc<dyn SecretStore + Send + Sync> =
        Arc::new(WindowsSecretStore::new(Environment::Test));
    let deps = sm_cli::CliDeps {
        store,
        launcher: Arc::new(ProcessLauncher),
    };
    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(_) => return ExitCode::from(EXIT_HOST),
    };
    if argv.iter().any(|token| token == "--self-check") {
        let code = runtime.block_on(run_self_check(&argv, &deps, &mut stdout(), &mut stderr()));
        return ExitCode::from(code);
    }
    let args = match parse_gui_args(&argv) {
        Ok(args) => args,
        Err(error) => {
            let message = match error {
                GuiArgError::UnknownArgument => "unknown argument",
                GuiArgError::MissingConfig => "an absolute --config path is required",
                GuiArgError::ConfigNotAbsolute => "the --config path must be absolute and local",
                GuiArgError::UnexpectedArgument => "an argument was repeated",
                GuiArgError::UnsupportedCombination => "--json is only valid with --self-check",
            };
            eprintln!("screenmemory-gui: {message}; the window is not started");
            return ExitCode::from(EXIT_USAGE);
        }
    };
    let config = match load_window_config(&args.config) {
        Ok(config) => config,
        Err(error) => {
            // The shared safe message only; never the path or a parse payload.
            eprintln!("screenmemory-gui: {}", sm_cli::config_message(error));
            return ExitCode::from(EXIT_USAGE);
        }
    };
    let webview_directory = config.root.join("webview");
    let session = match GuiSession::open(config, deps) {
        Ok(session) => session,
        Err(failure) => {
            eprintln!("screenmemory-gui: {}", failure.message);
            return ExitCode::from(failure.exit);
        }
    };
    let state = ShellState::new(session, Arc::new(SystemReportOpener), args.allow_start);
    match run_window(state, webview_directory) {
        Ok(()) => ExitCode::SUCCESS,
        Err(_) => ExitCode::from(EXIT_HOST),
    }
}
