//! `shotd` entry point: explicit arguments, safe stderr diagnostics, fixed exit codes.
use sm_application::credentials::SecretStore;
use sm_capture::CaptureHostConfig;
use sm_credentials::WindowsSecretStore;
use sm_host::{
    bootstrap, emit_pre_root_failure, parse_bootstrap_args, BootstrapMode, BootstrapOptions,
    CaptureOptions, EXIT_HOST, EXIT_USAGE,
};
use sm_isolation::Environment;
use std::{io::Write, process::ExitCode, sync::Arc};

fn main() -> ExitCode {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    let args = match parse_bootstrap_args(&argv) {
        Ok(args) => args,
        Err(_) => {
            eprintln!("shotd: usage error; expected --root <absolute local path> [--init]");
            return ExitCode::from(EXIT_USAGE);
        }
    };
    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(_) => return ExitCode::from(EXIT_HOST),
    };
    let store: Arc<dyn SecretStore + Send + Sync> =
        Arc::new(WindowsSecretStore::new(Environment::Test));
    let options = BootstrapOptions {
        root: args.root.clone(),
        mode: args.mode,
        store,
        // S02: the real Windows capture owner. It registers no system-wide hotkey until the
        // hotkey settings/tray owner exists, so this process claims no foreign chord.
        capture: CaptureOptions::Native(Box::new(CaptureHostConfig::new())),
    };
    runtime.block_on(async move {
        let host = match bootstrap(options).await {
            Ok(host) => host,
            Err(error) => {
                emit_pre_root_failure(error);
                return ExitCode::from(error.exit_code());
            }
        };
        match args.mode {
            BootstrapMode::Initialize => {
                let report = host.bootstrap_report();
                let outcome = host.shutdown().await;
                let line = serde_json::to_string(&report).expect("bounded bootstrap report");
                let mut stdout = std::io::stdout();
                let _ = stdout.write_all(line.as_bytes());
                let _ = stdout.write_all(b"\n");
                let _ = stdout.flush();
                ExitCode::from(outcome.exit_code())
            }
            BootstrapMode::Serve => match host.serve(sm_host::DEFAULT_DRAIN).await {
                Ok(outcome) => ExitCode::from(outcome.exit_code()),
                Err(error) => {
                    emit_pre_root_failure(error);
                    ExitCode::from(error.exit_code())
                }
            },
        }
    })
}
