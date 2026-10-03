//! `shotd` entry point: explicit arguments, safe stderr diagnostics, fixed exit codes.
use sm_application::credentials::{CredentialEnvironment, CredentialKey, SecretStore};
use sm_capture::CaptureHostConfig;
use sm_credentials::WindowsSecretStore;
use sm_host::{
    bootstrap_from_installation, emit_pre_root_failure, parse_bootstrap_args, BootstrapMode,
    BootstrapOptions, CaptureOptions, EXIT_HOST, EXIT_USAGE,
};
use sm_isolation::Environment;
use std::{
    io::{IsTerminal, Write},
    process::ExitCode,
    sync::Arc,
};

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
    let backend_secret = if args.enroll_runtime_token {
        if std::io::stdin().is_terminal() {
            return ExitCode::from(sm_host::EXIT_POLICY);
        }
        // A stalled nonterminal sender cannot hold enrollment indefinitely. No root/vault
        // authority has been acquired yet; on timeout this own process exits with code 8.
        let (sender, receiver) = std::sync::mpsc::sync_channel(1);
        let reader = std::thread::spawn(move || {
            let result = sm_host::read_backend_secret(&mut std::io::stdin().lock());
            let _ = sender.send(result);
        });
        match receiver.recv_timeout(std::time::Duration::from_secs(10)) {
            Ok(Ok(secret)) => {
                let _ = reader.join();
                Some(secret)
            }
            Ok(Err(error)) => {
                let _ = reader.join();
                emit_pre_root_failure(error);
                return ExitCode::from(error.exit_code());
            }
            Err(_) => {
                eprintln!("shotd: bounded secret input unavailable");
                return ExitCode::from(sm_host::EXIT_WAIT);
            }
        }
    } else {
        None
    };
    let enrollment_store = store.clone();
    let options = BootstrapOptions {
        root: args.root.clone(),
        mode: args.mode,
        store,
        // S02: the real Windows capture owner. It registers no system-wide hotkey until the
        // hotkey settings/tray owner exists, so this process claims no foreign chord.
        capture: CaptureOptions::Native(Box::new(CaptureHostConfig::new())),
    };
    runtime.block_on(async move {
        let host = match bootstrap_from_installation(options).await {
            Ok(host) => host,
            Err(error) => {
                emit_pre_root_failure(error);
                return ExitCode::from(error.exit_code());
            }
        };
        match args.mode {
            BootstrapMode::Initialize => {
                let report = host.bootstrap_report();
                if let Some(secret) = backend_secret {
                    let key = CredentialKey::runtime_backend(
                        CredentialEnvironment::Test,
                        sm_domain::LibraryId::new(report.library_id.as_uuid()),
                    );
                    if enrollment_store.create_new(&key, &secret).is_err() {
                        let _ = host.shutdown().await;
                        emit_pre_root_failure(sm_host::HostError::CredentialUnavailable);
                        return ExitCode::from(sm_host::EXIT_POLICY);
                    }
                }
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
