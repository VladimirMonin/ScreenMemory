use crate::*;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs::File,
    io::Read,
    path::{Component, Path, PathBuf},
    sync::atomic::{AtomicBool, Ordering},
    time::Duration,
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    process::{Child, ChildStdin, ChildStdout, Command},
    time::{timeout, Instant},
};
use tokio_util::sync::CancellationToken;

const MAX_FRAME: usize = 4 * 1024 * 1024;
static PROCESS_OWNED: AtomicBool = AtomicBool::new(false);
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Manifest {
    protocol: String,
    node_version: String,
    sdk_version: String,
    files: BTreeMap<String, String>,
}
/// Paths originate at a trusted install directory, never an RPC/config/environment path.
/// Pins every shipped module + runtime against writes for the lifetime of the supervisor.
pub fn private_node_command(executable: &Path) -> Result<Command, RuntimeError> {
    if !executable.is_absolute() {
        return Err(RuntimeError::settled("EXECUTABLE_REJECTED"));
    }
    let mut command = Command::new(executable);
    command
        .env_clear()
        .kill_on_drop(true)
        .arg("--no-global-search-paths")
        .arg("--no-addons");
    #[cfg(windows)]
    {
        command
            .env("SystemRoot", crate::windows::system_root()?)
            .creation_flags(0x08000000);
    }
    Ok(command)
}

pub struct BridgePackage {
    node: PathBuf,
    entry: PathBuf,
    _pins: Vec<File>,
}
impl BridgePackage {
    pub fn open(root: &Path) -> Result<Self, RuntimeError> {
        if !root.is_absolute() {
            return Err(RuntimeError::settled("PACKAGE_REJECTED"));
        }
        let manifest: Manifest = serde_json::from_slice(
            &std::fs::read(root.join("package-manifest.json"))
                .map_err(|_| RuntimeError::settled("PACKAGE_MISSING"))?,
        )
        .map_err(|_| RuntimeError::settled("PACKAGE_REJECTED"))?;
        if manifest.protocol != "sm-sdk/1.1"
            || manifest.node_version != "24.13.0"
            || manifest.sdk_version != "2.0.0"
            || manifest.files.len() > 4096
        {
            return Err(RuntimeError::settled("RUNTIME_COMPATIBILITY_CHANGED"));
        }
        let mut pins = Vec::new();
        for (name, hash) in &manifest.files {
            if Path::new(name)
                .components()
                .any(|c| !matches!(c, Component::Normal(_)))
                || hash.len() != 64
            {
                return Err(RuntimeError::settled("PACKAGE_REJECTED"));
            }
            let path = root.join(name);
            let mut ancestor = Some(path.as_path());
            while let Some(p) = ancestor {
                if std::fs::symlink_metadata(p)
                    .map_err(|_| RuntimeError::settled("PACKAGE_REJECTED"))?
                    .file_type()
                    .is_symlink()
                {
                    return Err(RuntimeError::settled("PACKAGE_REJECTED"));
                }
                #[cfg(windows)]
                {
                    use std::os::windows::fs::MetadataExt;
                    if std::fs::metadata(p)
                        .map_err(|_| RuntimeError::settled("PACKAGE_REJECTED"))?
                        .file_attributes()
                        & 0x400
                        != 0
                    {
                        return Err(RuntimeError::settled("PACKAGE_REJECTED"));
                    }
                }
                if p == root {
                    break;
                }
                ancestor = p.parent();
            }
            let mut opts = std::fs::OpenOptions::new();
            opts.read(true);
            #[cfg(windows)]
            {
                use std::os::windows::fs::OpenOptionsExt;
                opts.share_mode(1);
            }
            let mut file = opts
                .open(path)
                .map_err(|_| RuntimeError::settled("PACKAGE_REJECTED"))?;
            if file
                .metadata()
                .map_err(|_| RuntimeError::settled("PACKAGE_REJECTED"))?
                .len()
                > 128 * 1024 * 1024
            {
                return Err(RuntimeError::settled("PACKAGE_REJECTED"));
            }
            let mut hasher = Sha256::new();
            let mut buffer = [0u8; 8192];
            loop {
                let n = file
                    .read(&mut buffer)
                    .map_err(|_| RuntimeError::settled("PACKAGE_REJECTED"))?;
                if n == 0 {
                    break;
                }
                hasher.update(&buffer[..n]);
            }
            if format!("{:x}", hasher.finalize()) != *hash {
                return Err(RuntimeError::settled("PACKAGE_INTEGRITY_FAILED"));
            }
            pins.push(file);
        }
        for name in [
            "node.exe",
            "src/bridge.mjs",
            "src/sdk-adapter.ts",
            "node_modules/@lmstudio/sdk/dist/index.mjs",
        ] {
            if !manifest.files.contains_key(name) {
                return Err(RuntimeError::settled("PACKAGE_REJECTED"));
            }
        }
        Ok(Self {
            node: root.join("node.exe"),
            entry: root.join("src/bridge.mjs"),
            _pins: pins,
        })
    }
}
struct Connection {
    child: Child,
    stdin: ChildStdin,
    responses: tokio::sync::mpsc::Receiver<Result<Value, RuntimeError>>,
    reader: tokio::task::JoinHandle<()>,
    #[cfg(windows)]
    job: crate::windows::Job,
}
impl Connection {
    async fn stop(&mut self) -> Result<(), RuntimeError> {
        let _ = self.child.start_kill();
        timeout(Duration::from_secs(3), self.child.wait())
            .await
            .map_err(|_| RuntimeError::uncertain("TERMINATION_UNPROVEN"))?
            .map_err(|_| RuntimeError::uncertain("TERMINATION_UNPROVEN"))?;
        #[cfg(windows)]
        if !self.job.empty() {
            return Err(RuntimeError::uncertain("TERMINATION_UNPROVEN"));
        }
        self.reader.abort();
        Ok(())
    }
}
/// One owner, one serialized request + bounded cancellation control. Explicit restart never replays.
pub struct RuntimeSupervisor {
    package: BridgePackage,
    endpoint: String,
    connection: Option<Connection>,
    generation: uuid::Uuid,
    auth: Zeroizing<String>,
    next_id: u64,
    starts: u8,
    fenced: bool,
    lost_future: std::sync::Arc<AtomicBool>,
    owns_permit: bool,
}
impl RuntimeSupervisor {
    pub fn new(package: BridgePackage, endpoint: String) -> Result<Self, RuntimeError> {
        let port = endpoint.strip_prefix("ws://127.0.0.1:").and_then(|s| {
            if !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit()) {
                s.parse::<u16>().ok()
            } else {
                None
            }
        });
        if port.is_none_or(|p| p == 0) {
            return Err(RuntimeError::settled("ENDPOINT_REJECTED"));
        }
        if PROCESS_OWNED
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return Err(RuntimeError::settled("BRIDGE_ALREADY_OWNED"));
        }
        Ok(Self {
            package,
            endpoint,
            connection: None,
            generation: uuid::Uuid::nil(),
            auth: Zeroizing::new(String::new()),
            next_id: 1,
            starts: 0,
            fenced: false,
            lost_future: std::sync::Arc::new(AtomicBool::new(false)),
            owns_permit: true,
        })
    }
    /// Starts no model work. Policy Off can keep the supervisor unstarted. Max 3 starts per owner.
    pub async fn start(&mut self, secrets: &dyn SecretAuthority) -> Result<(), RuntimeError> {
        if self.connection.is_some() || self.starts >= 3 {
            return Err(RuntimeError::settled("RESTART_BUDGET_EXHAUSTED"));
        }
        self.starts += 1;
        self.generation = uuid::Uuid::new_v4();
        self.next_id = 1;
        let mut random = Zeroizing::new([0u8; 32]);
        getrandom::fill(&mut *random).map_err(|_| RuntimeError::settled("SECRET_UNAVAILABLE"))?;
        self.auth = Zeroizing::new(random.iter().map(|b| format!("{b:02x}")).collect());
        let token = secrets.lm_studio_token().await?;
        let mut command = private_node_command(&self.package.node)?;
        command
            .arg(&self.package.entry)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .kill_on_drop(true);
        #[cfg(windows)]
        command.creation_flags(0x08000000);
        let mut child = command
            .spawn()
            .map_err(|_| RuntimeError::settled("BRIDGE_UNAVAILABLE"))?;
        #[cfg(windows)]
        let job = match crate::windows::Job::new_and_assign(&child) {
            Ok(job) => job,
            Err(e) => {
                let _ = child.start_kill();
                if !matches!(
                    timeout(Duration::from_secs(3), child.wait()).await,
                    Ok(Ok(_))
                ) {
                    self.owns_permit = false;
                    return Err(RuntimeError::uncertain("TERMINATION_UNPROVEN"));
                }
                return Err(e);
            }
        };
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| RuntimeError::uncertain("BRIDGE_UNAVAILABLE"))?;
        let mut stdout = child
            .stdout
            .take()
            .ok_or_else(|| RuntimeError::uncertain("BRIDGE_UNAVAILABLE"))?;
        let (sender, responses) = tokio::sync::mpsc::channel(1);
        let reader = tokio::spawn(async move {
            loop {
                let response = read(&mut stdout).await;
                let failed = response.is_err();
                if sender.send(response).await.is_err() || failed {
                    break;
                }
            }
        });
        self.connection = Some(Connection {
            child,
            stdin,
            responses,
            reader,
            #[cfg(windows)]
            job,
        });
        let init = Zeroizing::new(serde_json::to_vec(&json!({ "protocol":"sm-sdk/1.1", "auth": &*self.auth, "generation":self.generation, "endpoint":self.endpoint, "api_token": &*token })).map_err(|_| RuntimeError::settled("INIT_INVALID"))?);
        let handshake = timeout(Duration::from_secs(3), async {
            let conn = self.connection.as_mut().unwrap();
            write(&mut conn.stdin, &init, &[]).await?;
            let response = conn
                .responses
                .recv()
                .await
                .ok_or_else(|| RuntimeError::uncertain("BRIDGE_IO"))??;
            if response["id"] != "0"
                || response["generation"] != self.generation.to_string()
                || response["result"]["protocol"] != "sm-sdk/1.1"
                || response["result"]["sdk"] != "2.0.0"
                || response["ok"] != true
            {
                return Err(RuntimeError::uncertain("RUNTIME_COMPATIBILITY_CHANGED"));
            }
            Ok(())
        })
        .await
        .unwrap_or_else(|_| Err(RuntimeError::uncertain("BRIDGE_HANDSHAKE_TIMEOUT")));
        if handshake.is_err() {
            self.fenced = true;
            self.shutdown().await?;
        }
        handshake
    }
    /// Stops only the owned Node/Job. Success is not LM Studio physical model eviction.
    pub async fn shutdown(&mut self) -> Result<(), RuntimeError> {
        if let Some(conn) = self.connection.as_mut() {
            conn.stop().await?;
        }
        self.connection = None;
        Ok(())
    }
    async fn call(
        &mut self,
        method: &str,
        params: Value,
        image: &[u8],
        cancel: CancellationToken,
        deadline: Duration,
    ) -> Result<Value, RuntimeError> {
        if deadline.is_zero() || deadline > Duration::from_secs(120) {
            return Err(RuntimeError::settled("DEADLINE_INVALID"));
        }
        if cancel.is_cancelled() {
            return Err(RuntimeError::settled("CANCELLED"));
        }
        let id = self.next_id.to_string();
        self.next_id += 1;
        let request = Zeroizing::new(serde_json::to_vec(&json!({ "id":id,"auth":&*self.auth,"generation":self.generation,"method":method,"params":params,"deadline_ms":deadline.as_millis() as u64,"image_length":image.len() })).map_err(|_| RuntimeError::settled("REQUEST_INVALID"))?);
        let conn = self
            .connection
            .as_mut()
            .ok_or_else(|| RuntimeError::settled("BRIDGE_NOT_STARTED"))?;
        let mut flight = FlightGuard {
            lost: self.lost_future.clone(),
            armed: true,
        };
        let end = Instant::now() + deadline;
        if !matches!(
            timeout(deadline, write(&mut conn.stdin, &request, image)).await,
            Ok(Ok(()))
        ) {
            self.fenced = true;
            self.shutdown().await?;
            return Err(RuntimeError::uncertain("WRITE_TIMEOUT"));
        }
        // Bounded persistent pump owns partial frames; select never cancels raw stream reads.
        let mut cancelled = false;
        let mut cancel_ack = false;
        let mut original = None;
        let mut grace = end;
        let outcome = loop {
            tokio::select! {
                response = conn.responses.recv() => {
                    let mut response = match response {
                        Some(Ok(value)) => value,
                        Some(Err(error)) => break Err(error),
                        None => break Err(RuntimeError::uncertain("BRIDGE_IO")),
                    };
                    if response["generation"] != self.generation.to_string() { break Err(RuntimeError::uncertain("GENERATION_MISMATCH")); }
                    if response["id"] != id {
                        if cancelled && !cancel_ack && response["id"].as_str().and_then(|s| s.parse::<u64>().ok()) == Some(self.next_id-1) {
                            cancel_ack = true;
                            if let Some(outcome) = original.take() { break outcome; }
                            continue;
                        }
                        break Err(RuntimeError::uncertain("CORRELATION_MISMATCH"));
                    }
                    let result = if response["ok"] == true {
                        if cancelled && method == "vision" { response["result"]["complete"] = json!(false); response["result"]["value"] = Value::Null; }
                        Ok(response["result"].take())
                    } else {
                        let settlement = if response["error"]["settlement"] == "settled" { Settlement::Settled } else { Settlement::Uncertain };
                        Err(RuntimeError { code: safe_code(response["error"]["code"].as_str().unwrap_or("SDK_FAILURE")), settlement })
                    };
                    if cancelled && !cancel_ack { original = Some(result); } else { break result; }
                },
                _ = async {
                    if cancelled { tokio::time::sleep_until(grace).await; }
                    else { tokio::select! { _ = cancel.cancelled() => {}, _ = tokio::time::sleep_until(grace) => {} } }
                } => {
                    if cancelled { break Err(RuntimeError::uncertain("DEADLINE_UNCERTAIN")); }
                    cancelled = true; grace = Instant::now()+Duration::from_secs(3);
                    let cancel_id = self.next_id.to_string(); self.next_id += 1;
                    let control = Zeroizing::new(serde_json::to_vec(&json!({"id":cancel_id,"auth":&*self.auth,"generation":self.generation,"method":"cancel","params":{"target":id},"deadline_ms":2000,"image_length":0})).unwrap());
                    if !matches!(timeout(Duration::from_millis(500),write(&mut conn.stdin,&control,&[])).await, Ok(Ok(()))) { break Err(RuntimeError::uncertain("CANCEL_UNCERTAIN")); }
                }
            }
        };
        if outcome
            .as_ref()
            .is_err_and(|e| e.settlement == Settlement::Uncertain)
        {
            self.fenced = true;
            self.shutdown().await?;
        }
        flight.armed = false;
        outcome
    }
    pub async fn inventory(
        &mut self,
        ctx: &RequestContext,
        cancel: CancellationToken,
    ) -> Result<Value, RuntimeError> {
        require(ctx, Scope::ReadRuntimeInventory)?;
        self.call("inventory", json!({}), &[], cancel, Duration::from_secs(10))
            .await
    }
    pub async fn load_owned(
        &mut self,
        ctx: &RequestContext,
        ai_enabled: bool,
        plan: LoadPlan,
        store: &dyn LoadIntentStore,
        cancel: CancellationToken,
    ) -> Result<OwnedInstance, RuntimeError> {
        require(ctx, Scope::LoadModelForProbe)?;
        if !ai_enabled || self.fenced || self.lost_future.load(Ordering::Acquire) {
            return Err(RuntimeError::settled(
                if self.fenced || self.lost_future.load(Ordering::Acquire) {
                    "RECONCILIATION_REQUIRED"
                } else {
                    "AI_OFF"
                },
            ));
        }
        if plan.intent_id.is_nil()
            || plan.identifier != format!("sm-{}", plan.intent_id)
            || plan.model_key.is_empty()
            || plan.model_key.len() > 512
            || !(1024..=262144).contains(&plan.context_length)
            || !(1..=2048).contains(&plan.eval_batch_size)
            || plan.ttl_seconds.is_some_and(|s| !(1..=86400).contains(&s))
            || (!matches!(plan.v_cache_type, CacheType::F16 | CacheType::F32)
                && !plan.flash_attention)
        {
            return Err(RuntimeError::settled("LOAD_INTENT_INVALID"));
        }
        store
            .commit_before_dispatch(&self.endpoint, self.generation, &plan)
            .await?;
        let outcome = self
            .call(
                "load",
                serde_json::to_value(&plan).unwrap(),
                &[],
                cancel,
                Duration::from_secs(120),
            )
            .await
            .and_then(|v| {
                serde_json::from_value::<LoadObservation>(v)
                    .map_err(|_| RuntimeError::uncertain("LOAD_RESPONSE_INVALID"))
            });
        if store
            .record_outcome(plan.intent_id, &outcome)
            .await
            .is_err()
        {
            self.fenced = true;
            return Err(RuntimeError::uncertain("LOAD_OUTCOME_NOT_PERSISTED"));
        }
        let observation = outcome?;
        if observation.intent_id != plan.intent_id
            || observation.model_key != plan.model_key
            || observation.identifier != plan.identifier
            || observation.configuration != "reported_applied"
            || observation.native_ttl.requested_seconds != plan.ttl_seconds
            || observation.native_ttl.verification != "not_verified"
        {
            self.fenced = true;
            return Err(RuntimeError::uncertain("CONFIGURATION_MISMATCH"));
        }
        Ok(OwnedInstance {
            observation,
            generation: self.generation,
        })
    }
    pub async fn predict_structured(
        &mut self,
        ctx: &RequestContext,
        ai_enabled: bool,
        instance: &OwnedInstance,
        image: &PreparedInferenceImage,
        request: VisionRequest,
        cancel: CancellationToken,
    ) -> Result<Prediction, RuntimeError> {
        require(ctx, Scope::RunAiProbe)?;
        require(ctx, Scope::ReadImages)?;
        if !ai_enabled
            || self.fenced
            || self.lost_future.load(Ordering::Acquire)
            || instance.generation != self.generation
        {
            return Err(RuntimeError::settled("DISPATCH_REVOKED"));
        }
        if request.max_tokens == 0
            || request.max_tokens > 4000
            || request.prompt.is_empty()
            || request.prompt.len() > 32768
            || !request.temperature.is_finite()
            || !(0.0..=1.0).contains(&request.temperature)
            || !request.schema.is_object()
        {
            return Err(RuntimeError::settled("TOKEN_LIMIT_INVALID"));
        }
        let params = json!({ "instance_reference":instance.observation.instance_reference,"prompt":request.prompt,"schema":request.schema,"max_tokens":request.max_tokens,"temperature":request.temperature,"reasoning":request.reasoning,"width":image.width,"height":image.height,"max_side":image.max_side,"sha256":image.hash });
        let value = self
            .call(
                "vision",
                params,
                &image.bytes,
                cancel,
                Duration::from_secs(120),
            )
            .await?;
        let result: Prediction = serde_json::from_value(value)
            .map_err(|_| RuntimeError::uncertain("PREDICTION_INVALID"))?;
        if result.instance_reference != instance.observation.instance_reference
            || result
                .usage
                .generated_tokens
                .is_some_and(|n| n > request.max_tokens as u64)
        {
            self.fenced = true;
            return Err(RuntimeError::uncertain("PREDICTION_INVALID"));
        }
        Ok(result)
    }
    pub async fn observe(
        &mut self,
        instance: &OwnedInstance,
        cancel: CancellationToken,
    ) -> Result<Value, RuntimeError> {
        if instance.generation != self.generation {
            return Err(RuntimeError::settled("STALE_INSTANCE"));
        }
        self.call(
            "observe",
            json!({"instance_reference":instance.observation.instance_reference}),
            &[],
            cancel,
            Duration::from_secs(10),
        )
        .await
    }
    pub fn unload_owned(&mut self, _instance: &OwnedInstance) -> Result<(), RuntimeError> {
        Err(RuntimeError::settled(
            "UNLOAD_IDENTITY_ATOMICITY_UNSUPPORTED",
        ))
    }
}
struct FlightGuard {
    lost: std::sync::Arc<AtomicBool>,
    armed: bool,
}
impl Drop for FlightGuard {
    fn drop(&mut self) {
        if self.armed {
            self.lost.store(true, Ordering::Release);
        }
    }
}
impl Drop for RuntimeSupervisor {
    fn drop(&mut self) {
        // Job kill-on-close/Child kill-on-drop. Do not release the singleton without proven wait.
        if self.connection.is_none() && self.owns_permit {
            PROCESS_OWNED.store(false, Ordering::Release);
        }
    }
}
async fn write(stdin: &mut ChildStdin, json: &[u8], image: &[u8]) -> Result<(), RuntimeError> {
    if json.len() > MAX_FRAME {
        return Err(RuntimeError::settled("FRAME_LIMIT"));
    }
    stdin
        .write_u32_le(json.len() as u32)
        .await
        .map_err(|_| RuntimeError::uncertain("BRIDGE_IO"))?;
    stdin
        .write_all(json)
        .await
        .map_err(|_| RuntimeError::uncertain("BRIDGE_IO"))?;
    stdin
        .write_all(image)
        .await
        .map_err(|_| RuntimeError::uncertain("BRIDGE_IO"))?;
    stdin
        .flush()
        .await
        .map_err(|_| RuntimeError::uncertain("BRIDGE_IO"))
}
async fn read(stdout: &mut ChildStdout) -> Result<Value, RuntimeError> {
    let n = stdout
        .read_u32_le()
        .await
        .map_err(|_| RuntimeError::uncertain("BRIDGE_IO"))? as usize;
    if n == 0 || n > MAX_FRAME {
        return Err(RuntimeError::uncertain("FRAME_LIMIT"));
    }
    let mut bytes = Zeroizing::new(vec![0u8; n]);
    stdout
        .read_exact(&mut bytes)
        .await
        .map_err(|_| RuntimeError::uncertain("BRIDGE_IO"))?;
    serde_json::from_slice(&bytes).map_err(|_| RuntimeError::uncertain("BRIDGE_PROTOCOL"))
}
#[async_trait]
impl ModelLifecycle for RuntimeSupervisor {
    async fn inventory(
        &mut self,
        ctx: &RequestContext,
        cancel: CancellationToken,
    ) -> Result<RuntimeInventory, RuntimeError> {
        let value = RuntimeSupervisor::inventory(self, ctx, cancel).await?;
        serde_json::from_value(value).map_err(|_| RuntimeError::uncertain("INVENTORY_INVALID"))
    }
    async fn load_owned(
        &mut self,
        ctx: &RequestContext,
        ai_enabled: bool,
        plan: LoadPlan,
        store: &dyn LoadIntentStore,
        cancel: CancellationToken,
    ) -> Result<OwnedInstance, RuntimeError> {
        RuntimeSupervisor::load_owned(self, ctx, ai_enabled, plan, store, cancel).await
    }
    fn unload_owned(&mut self, instance: &OwnedInstance) -> Result<(), RuntimeError> {
        RuntimeSupervisor::unload_owned(self, instance)
    }
}
#[async_trait]
impl VisionInference for RuntimeSupervisor {
    async fn predict_structured(
        &mut self,
        ctx: &RequestContext,
        ai_enabled: bool,
        instance: &OwnedInstance,
        image: &PreparedInferenceImage,
        request: VisionRequest,
        cancel: CancellationToken,
    ) -> Result<Prediction, RuntimeError> {
        RuntimeSupervisor::predict_structured(
            self, ctx, ai_enabled, instance, image, request, cancel,
        )
        .await
    }
}
fn safe_code(code: &str) -> &'static str {
    match code {
        "NOT_OWNED" => "NOT_OWNED",
        "IDENTIFIER_COLLISION" => "IDENTIFIER_COLLISION",
        "LOAD_UNCERTAIN" => "LOAD_UNCERTAIN",
        "REASONING_UNSUPPORTED" => "REASONING_UNSUPPORTED",
        "STRUCTURED_INVALID" => "STRUCTURED_INVALID",
        "OUTPUT_LIMIT" => "OUTPUT_LIMIT",
        "DEADLINE_UNCERTAIN" => "DEADLINE_UNCERTAIN",
        "CANCELLED" => "CANCELLED",
        "BUSY" => "BUSY",
        "UNLOAD_IDENTITY_ATOMICITY_UNSUPPORTED" => "UNLOAD_IDENTITY_ATOMICITY_UNSUPPORTED",
        _ => "SDK_FAILURE",
    }
}
