#![cfg(windows)]
use async_trait::async_trait;
use sm_application::{images::ImageLibraryApi, RequestContext, Scope};
use sm_domain::{CorrelationId, ImportBatchId, OperationId, PrincipalId, RequestId};
use sm_isolation::{HostLease, LibraryRootResolver, RootSource};
use sm_protocol::images::{CanonicalProfile, DuplicatePolicy, ImportBatchRequest, ImportRequest};
use sm_runtime::*;
use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;
use zeroize::Zeroizing;

struct Secrets;
#[async_trait]
impl SecretAuthority for Secrets {
    async fn lm_studio_token(&self) -> Result<Zeroizing<String>, RuntimeError> {
        Ok(Zeroizing::new("sk-lm-syntest1:01234567890123456789".into()))
    }
}
struct Journal(PathBuf);
#[async_trait]
impl LoadIntentStore for Journal {
    async fn commit_before_dispatch(
        &self,
        endpoint: &str,
        generation: Uuid,
        plan: &LoadPlan,
    ) -> Result<(), RuntimeError> {
        let mut f = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&self.0)
            .unwrap();
        serde_json::to_writer(
            &mut f,
            &serde_json::json!({"endpoint":endpoint,"generation":generation,"intent":plan}),
        )
        .unwrap();
        f.sync_all().unwrap();
        Ok(())
    }
    async fn record_outcome(
        &self,
        intent: Uuid,
        outcome: &Result<LoadObservation, RuntimeError>,
    ) -> Result<(), RuntimeError> {
        let value: serde_json::Value = serde_json::from_slice(&fs::read(&self.0).unwrap()).unwrap();
        assert_eq!(value["intent"]["intent_id"], intent.to_string());
        let mut f = fs::OpenOptions::new().append(true).open(&self.0).unwrap();
        writeln!(
            f,
            "\n{}",
            if outcome.is_ok() {
                "observed"
            } else {
                "uncertain"
            }
        )
        .unwrap();
        f.sync_all().unwrap();
        Ok(())
    }
}
fn ctx() -> RequestContext {
    RequestContext::from_authenticated_peer(
        RequestId::new(Uuid::new_v4()),
        CorrelationId::new(Uuid::new_v4()),
        PrincipalId::new(Uuid::new_v4()),
        vec![
            Scope::ReadRuntimeInventory,
            Scope::RunAiProbe,
            Scope::LoadModelForProbe,
            Scope::ReadImages,
            Scope::ImportFiles,
            Scope::ExportImages,
        ],
    )
}
fn own_root() -> PathBuf {
    let parent = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../.pi/temporary");
    let parent = fs::canonicalize(parent).unwrap();
    let path = PathBuf::from(parent.to_string_lossy().trim_start_matches(r"\\?\"))
        .join(Uuid::new_v4().to_string());
    fs::create_dir(&path).unwrap();
    path
}
#[tokio::test(flavor = "current_thread")]
async fn s04_native_package_s03_canonical_lease_sdk_and_owned_restart() {
    let canonical_repo = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap();
    let repo = PathBuf::from(canonical_repo.to_string_lossy().trim_start_matches(r"\\?\"));
    let node = repo.join(".pi/toolchains/node-v24.13.0-win-x64/node.exe");
    // Poison only this harness-owned test process, never the shell/registry/global environment.
    std::env::set_var("SystemRoot", r"C:\synthetic-poison");
    std::env::set_var("NODE_OPTIONS", "--require invalid-synthetic-module");
    std::env::set_var("NODE_PATH", "synthetic-poison");
    std::env::set_var("LM_API_TOKEN", "synthetic-poison");
    let env_probe = private_node_command(&node)
        .unwrap()
        .arg("-e")
        .arg("process.stdout.write(JSON.stringify(process.env))")
        .output()
        .await
        .unwrap();
    assert!(env_probe.status.success());
    let env_value: serde_json::Value = serde_json::from_slice(&env_probe.stdout).unwrap();
    assert_eq!(env_value.as_object().unwrap().len(), 1);
    assert!(env_value["SystemRoot"]
        .as_str()
        .is_some_and(|s| s != r"C:\synthetic-poison"));
    let package_root = own_root(); // stage requires a new output, so use an absent child of own UUID.
    let package_path = package_root.join("package");
    let status = private_node_command(&node)
        .unwrap()
        .arg(repo.join("packages/lmstudio-bridge/scripts/stage.mjs"))
        .arg(&package_path)
        .output()
        .await
        .unwrap();
    if !status.status.success() {
        fs::write(package_root.join("stage-stderr.txt"), &status.stderr).unwrap();
        panic!(
            "stage failed; own evidence UUID {}",
            package_root.file_name().unwrap().to_string_lossy()
        );
    }
    let package = BridgePackage::open(&package_path).unwrap();
    let mut server = private_node_command(&node)
        .unwrap()
        .arg(repo.join("packages/lmstudio-bridge/test/synthetic-endpoint.mjs"))
        .arg("--stdio-fixture")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    let mut lines = BufReader::new(server.stdout.take().unwrap()).lines();
    let endpoint = tokio::time::timeout(Duration::from_secs(3), lines.next_line())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let mut runtime = RuntimeSupervisor::new(package, endpoint).unwrap();
    let context = ctx();
    runtime.start(&Secrets).await.unwrap();
    assert_eq!(
        runtime
            .inventory(&context, CancellationToken::new())
            .await
            .unwrap()["downloaded"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    let id = Uuid::new_v4();
    let plan = LoadPlan {
        intent_id: id,
        identifier: format!("sm-{id}"),
        model_key: "synthetic/vision.gguf".into(),
        context_length: 8192,
        eval_batch_size: 256,
        flash_attention: true,
        offload_kv_cache_to_gpu: true,
        k_cache_type: CacheType::F16,
        v_cache_type: CacheType::Q8_0,
        ttl_seconds: Some(900),
    };
    let journal = Journal(package_root.join("load-intent.json"));
    assert_eq!(
        runtime
            .load_owned(
                &context,
                false,
                plan.clone(),
                &journal,
                CancellationToken::new()
            )
            .await
            .err()
            .unwrap()
            .code,
        "AI_OFF"
    );
    assert!(!journal.0.exists());
    let instance = runtime
        .load_owned(&context, true, plan, &journal, CancellationToken::new())
        .await
        .unwrap();
    assert!(fs::read_to_string(&journal.0)
        .unwrap()
        .ends_with("observed\n"));
    assert_eq!(
        instance.observation().native_ttl.requested_seconds,
        Some(900)
    );
    assert_eq!(
        instance.observation().native_ttl.verification,
        "not_verified"
    );
    assert_eq!(
        runtime.unload_owned(&instance).err().unwrap().code,
        "UNLOAD_IDENTITY_ATOMICITY_UNSUPPORTED"
    );

    let library = own_root();
    for child in ["data", "config", "cache", "webview"] {
        fs::create_dir(library.join(child)).unwrap();
    }
    let storage = sm_storage::StorageWorker::start(
        HostLease::acquire(
            LibraryRootResolver::resolve(RootSource::Test {
                root: &library,
                forbidden_donor_roots: &[],
            })
            .unwrap(),
        )
        .unwrap(),
    )
    .unwrap();
    let worker_path = repo.join(".pi/staging-target-t09b/debug/shot-image-worker.exe");
    let worker_hash = sha2_digest(&fs::read(&worker_path).unwrap());
    let worker =
        Arc::new(sm_image_worker::TrustedWorker::from_manifest(&worker_path, worker_hash).unwrap());
    let images = sm_storage::image_services::ImageServices::new(storage.clone(), worker);
    let source = package_root.join("input.png");
    fs::write(
        &source,
        include_bytes!("../../../tests/fixtures/images/odd-alpha.png"),
    )
    .unwrap();
    let batch = ImportBatchId::new(Uuid::new_v4());
    images
        .import_paths(
            context.clone(),
            ImportRequest {
                batch_id: batch,
                source_paths: vec![source.to_string_lossy().into_owned()],
                profile: CanonicalProfile::WebpExact,
                duplicates: DuplicatePolicy::KeepBoth,
            },
        )
        .await
        .unwrap();
    let report = loop {
        let report = images
            .import_status(context.clone(), ImportBatchRequest { batch_id: batch })
            .await
            .unwrap();
        if report.discovery_complete && report.in_flight == 0 && report.saved_new == 1 {
            break report;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    };
    let receipt = report.items[0].receipt.as_ref().unwrap();
    fs::remove_file(source).unwrap(); // inference never rereads the deleted external input.
    let canonical = fs::read(library.join(&receipt.relative_path)).unwrap();
    for max_side in [1024, 2048] {
        let image = PreparedInferenceImage::prepare(
            &images,
            context.clone(),
            OperationId::new(Uuid::new_v4()),
            receipt.record_id,
            receipt.original_asset_id,
            max_side,
        )
        .await
        .unwrap();
        let request = VisionRequest {
            prompt: "Synthetic technical fixture only".into(),
            schema: serde_json::json!({"type":"object","additionalProperties":false,"required":["title"],"properties":{"title":{"type":"string"}}}),
            max_tokens: 4000,
            temperature: 0.2,
            reasoning: Reasoning::BackendDefault,
        };
        let prediction = runtime
            .predict_structured(
                &context,
                true,
                &instance,
                &image,
                request,
                CancellationToken::new(),
            )
            .await
            .unwrap();
        assert!(prediction.complete);
        assert_eq!(prediction.value.unwrap()["title"], "Synthetic only");
        assert_eq!(
            fs::read(library.join(&receipt.relative_path)).unwrap(),
            canonical
        );
    }
    while lines.next_line().await.unwrap().unwrap() != "predict_received" {}
    while lines.next_line().await.unwrap().unwrap() != "predict_received" {}
    server
        .stdin
        .as_mut()
        .unwrap()
        .write_all(b"hang\n")
        .await
        .unwrap();
    assert_eq!(lines.next_line().await.unwrap().unwrap(), "mode_set");
    let held_image = PreparedInferenceImage::prepare(
        &images,
        context.clone(),
        OperationId::new(Uuid::new_v4()),
        receipt.record_id,
        receipt.original_asset_id,
        1024,
    )
    .await
    .unwrap();
    let vision_request = || VisionRequest {
        prompt: "Synthetic technical fixture only".into(),
        schema: serde_json::json!({"type":"object","required":["title"],"properties":{"title":{"type":"string"}}}),
        max_tokens: 4000,
        temperature: 0.2,
        reasoning: Reasoning::BackendDefault,
    };
    let cancellation = CancellationToken::new();
    let cancel_when_dispatched = async {
        assert_eq!(
            lines.next_line().await.unwrap().unwrap(),
            "predict_received"
        );
        cancellation.cancel();
    };
    let (cancelled, ()) = tokio::join!(
        runtime.predict_structured(
            &context,
            true,
            &instance,
            &held_image,
            vision_request(),
            cancellation.clone()
        ),
        cancel_when_dispatched
    );
    let cancelled = cancelled.unwrap();
    assert_eq!(cancelled.stop_reason, "userStopped");
    assert!(!cancelled.complete);
    // Both ACK and final response are drained; the next request has no stale correlation.
    runtime
        .inventory(&context, CancellationToken::new())
        .await
        .unwrap();
    server
        .stdin
        .as_mut()
        .unwrap()
        .write_all(b"ignore_cancel\n")
        .await
        .unwrap();
    assert_eq!(lines.next_line().await.unwrap().unwrap(), "mode_set");
    let cancellation = CancellationToken::new();
    let cancel_when_dispatched = async {
        assert_eq!(
            lines.next_line().await.unwrap().unwrap(),
            "predict_received"
        );
        cancellation.cancel();
    };
    let (uncertain, ()) = tokio::join!(
        runtime.predict_structured(
            &context,
            true,
            &instance,
            &held_image,
            vision_request(),
            cancellation.clone()
        ),
        cancel_when_dispatched
    );
    assert_eq!(uncertain.err().unwrap().settlement, Settlement::Uncertain);
    // Caller keeps the accepted derivative/lease across uncertainty until external settlement.
    assert_eq!(
        fs::read(library.join(&receipt.relative_path)).unwrap(),
        canonical
    );
    runtime.shutdown().await.unwrap();
    runtime.start(&Secrets).await.unwrap();
    assert_eq!(
        runtime
            .observe(&instance, CancellationToken::new())
            .await
            .err()
            .unwrap()
            .code,
        "STALE_INSTANCE"
    );
    // Restart never adopts even the same identifier/immutable ref from surviving synthetic server.
    assert_eq!(
        runtime
            .inventory(&context, CancellationToken::new())
            .await
            .unwrap()["loaded"][0]["owned"],
        false
    );
    server.start_kill().unwrap();
    server.wait().await.unwrap();
    drop(held_image); // This harness-owned backend has now actually terminated.
    assert_eq!(
        runtime
            .inventory(&context, CancellationToken::new())
            .await
            .err()
            .unwrap()
            .settlement,
        Settlement::Uncertain
    );
    runtime.shutdown().await.unwrap();
    drop(runtime);
    storage.shutdown(Duration::from_secs(20)).await.unwrap();
    drop(images);
    drop(storage);
    // Only this test's own UUID roots after all native owners are closed.
    fs::remove_dir_all(library).unwrap();
    fs::remove_dir_all(package_root).unwrap();
}
fn sha2_digest(bytes: &[u8]) -> [u8; 32] {
    use sha2::Digest;
    sha2::Sha256::digest(bytes).into()
}
