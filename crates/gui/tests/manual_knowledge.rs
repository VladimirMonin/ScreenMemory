#![cfg(windows)]
//! Default shotd + JSON CLI + real GuiSession parity on owned synthetic library, AI Off.
use sha2::{Digest, Sha256};
use sm_application::credentials::{CredentialEnvironment, CredentialKey, SecretStore, StoreError};
use sm_cli::{CliDeps, ProcessLauncher};
use sm_credentials::WindowsSecretStore;
use sm_domain::LibraryId;
use sm_gui::{load_window_config, GuiSession};
use sm_isolation::Environment;
use sm_protocol::knowledge::*;
use std::{
    fs,
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::{
        atomic::{AtomicU32, Ordering},
        Arc, OnceLock,
    },
    time::{Duration, Instant},
};
use uuid::Uuid;
static EVIDENCE: OnceLock<PathBuf> = OnceLock::new();
static COMMAND_NUMBER: AtomicU32 = AtomicU32::new(0);
struct Owned {
    root: PathBuf,
    source_dir: PathBuf,
    host: Option<Child>,
    key: Option<CredentialKey>,
}
impl Owned {
    fn stop_wait(&mut self) {
        if let Some(mut child) = self.host.take() {
            let until = Instant::now() + Duration::from_secs(15);
            loop {
                if child.try_wait().unwrap().is_some() {
                    break;
                }
                if Instant::now() >= until {
                    child.kill().unwrap();
                    child.wait().unwrap();
                    break;
                }
                std::thread::sleep(Duration::from_millis(20));
            }
        }
    }
}
impl Drop for Owned {
    fn drop(&mut self) {
        self.stop_wait();
        if let Some(key) = self.key.take() {
            let vault = WindowsSecretStore::new(Environment::Test);
            if vault.read(&key).is_ok() {
                let system = std::env::var_os("SystemRoot").expect("native prerequisite");
                let status = Command::new(PathBuf::from(system).join("System32/cmdkey.exe"))
                    .arg(format!("/delete:{}", key.target()))
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .status()
                    .expect("exact own cleanup");
                assert!(status.success());
            }
            assert!(matches!(vault.read(&key), Err(StoreError::Missing)));
            println!("own exact owner credential absence confirmed");
        }
        fs::remove_dir_all(&self.root).unwrap();
        fs::remove_dir_all(&self.source_dir).unwrap();
    }
}
fn json(exe: &Path, config: Option<&Path>, args: &[String], exit: i32) -> serde_json::Value {
    let mut c = Command::new(exe);
    if let Some(config) = config {
        c.arg("--config").arg(config);
    }
    c.args(args);
    let out = c.output().unwrap();
    let n = COMMAND_NUMBER.fetch_add(1, Ordering::SeqCst);
    let evidence = EVIDENCE.get().expect("owned evidence");
    fs::write(evidence.join(format!("command-{n}.stdout")), &out.stdout).unwrap();
    fs::write(evidence.join(format!("command-{n}.stderr")), &out.stderr).unwrap();
    fs::write(
        evidence.join(format!("command-{n}.json")),
        serde_json::to_vec(&serde_json::json!({"arguments": args, "raw_exit": out.status.code()}))
            .unwrap(),
    )
    .unwrap();
    println!(
        "native {} raw_exit={:?}",
        args.first().unwrap(),
        out.status.code()
    );
    assert_eq!(out.status.code(), Some(exit));
    assert_eq!(std::str::from_utf8(&out.stdout).unwrap().lines().count(), 1);
    serde_json::from_slice(&out.stdout).unwrap()
}
fn strings(args: &[&str]) -> Vec<String> {
    args.iter().map(|s| (*s).into()).collect()
}
#[test]
fn s05_actual_default_shotd_cli_gui_manual_replay_conflict_reopen_export_ai_off() {
    let repo = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap();
    let repo = PathBuf::from(repo.to_string_lossy().trim_start_matches(r"\\?\"));
    let install = repo.join(".pi/staging-target-t09b/debug");
    let temporary = repo.join(".pi").join("temporary");
    let evidence = temporary.join(Uuid::new_v4().to_string());
    fs::create_dir(&evidence).unwrap();
    println!(
        "owned native immutable evidence UUID={}",
        evidence.file_name().unwrap().to_str().unwrap()
    );
    EVIDENCE.set(evidence.clone()).unwrap();
    let container = temporary.join(Uuid::new_v4().to_string());
    fs::create_dir(&container).unwrap();
    let root = container.join("root");
    fs::create_dir(&root).unwrap();
    for c in ["data", "config", "cache", "webview"] {
        fs::create_dir(root.join(c)).unwrap();
    }
    println!(
        "owned native root UUID={}",
        container.file_name().unwrap().to_str().unwrap()
    );
    let source_dir = temporary.join(Uuid::new_v4().to_string());
    fs::create_dir(&source_dir).unwrap();
    let mut owned = Owned {
        root: container.clone(),
        source_dir: source_dir.clone(),
        host: None,
        key: None,
    };
    let boot = json(
        &install.join("shotd.exe"),
        None,
        &strings(&["--root", root.to_str().unwrap(), "--init"]),
        0,
    );
    let library = LibraryId::new(Uuid::parse_str(boot["library_id"].as_str().unwrap()).unwrap());
    owned.key = Some(CredentialKey::owner(CredentialEnvironment::Test, library));
    assert_eq!(boot["schema_version"], 7);
    println!("owned library UUID={}", library.as_uuid());
    let config = root.join("config/client.json");
    fs::write(&config,serde_json::to_vec(&serde_json::json!({"schema_version":1,"environment":"test","root":root,"library_id":library.as_uuid(),"host_executable":install.join("shotd.exe")})).unwrap()).unwrap();
    let launch = |o: &mut Owned| {
        let child = Command::new(install.join("shotd.exe"))
            .args(["--root", root.to_str().unwrap()])
            .stdout(Stdio::null())
            .stderr(Stdio::from(
                fs::File::create(evidence.join(format!("host-{}.stderr", Uuid::new_v4()))).unwrap(),
            ))
            .spawn()
            .unwrap();
        println!("owned shotd child pid={} handle-held=true", child.id());
        o.host = Some(child);
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let out = Command::new(install.join("shot.exe"))
                .arg("--config")
                .arg(&config)
                .args(["status", "--no-start", "--json"])
                .output()
                .unwrap();
            if out.status.success() {
                break;
            }
            assert!(
                o.host.as_mut().unwrap().try_wait().unwrap().is_none(),
                "owned host exited before ready"
            );
            assert!(Instant::now() < deadline, "owned host readiness timeout");
            std::thread::sleep(Duration::from_millis(30));
        }
    };
    launch(&mut owned);
    let shot = |args: &[String], exit| json(&install.join("shot.exe"), Some(&config), args, exit);
    let status = shot(&strings(&["status", "--no-start", "--json"]), 0);
    assert_eq!(status["ai_enabled"], false);
    assert_eq!(status["bridge_state"], "stopped");
    let source = source_dir.join("synthetic.png");
    fs::write(
        &source,
        include_bytes!("../../../tests/fixtures/images/odd-alpha.png"),
    )
    .unwrap();
    let imported = shot(
        &strings(&[
            "import",
            "--path",
            source.to_str().unwrap(),
            "--wait-ms",
            "30000",
            "--json",
        ]),
        0,
    );
    fs::remove_file(source).unwrap();
    let rid = imported["items"][0]["receipt"]["record_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let id = KnowledgeId(Uuid::parse_str(&rid).unwrap());
    let original = fs::read(
        root.join("assets")
            .read_dir()
            .unwrap()
            .next()
            .unwrap()
            .unwrap()
            .path()
            .join(&rid)
            .join("original.webp"),
    )
    .unwrap();
    let canonical_sha = format!("{:x}", Sha256::digest(&original));
    let metadata = shot(&strings(&["metadata", "--record-id", &rid]), 0);
    let create = CategoryMutation {
        operation_id: KnowledgeId(Uuid::new_v4()),
        expected_catalog_revision: SqlRevision(1),
        change: CategoryChange::Create {
            name: CategoryName("Ручная категория".into()),
            description: CategoryDescription("synthetic".into()),
        },
    };
    let category = shot(
        &strings(&[
            "knowledge",
            "category",
            "--request-json",
            &serde_json::to_string(&create).unwrap(),
        ]),
        0,
    );
    let cid = category["value"]["category"]["id"].as_str().unwrap();
    let operation = Uuid::new_v4().to_string();
    let args = strings(&[
        "knowledge",
        "edit",
        "--record-id",
        &rid,
        "--operation-id",
        &operation,
        "--expected-revision",
        "1",
        "--catalog-revision",
        "2",
        "--category-revision",
        "1",
        "--category-id",
        cid,
        "--title",
        "Карточка AI Off",
        "--description",
        "Сохранено без модели",
    ]);
    let result = shot(&args, 0);
    assert_eq!(result, shot(&args, 0));
    let mut changed = args.clone();
    *changed.last_mut().unwrap() = "changed parameters".into();
    assert_eq!(shot(&changed, 5)["code"], "IDEMPOTENCY_CONFLICT");
    let mut stale = args.clone();
    stale[5] = Uuid::new_v4().to_string();
    assert_eq!(shot(&stale, 5)["code"], "REVISION_CONFLICT");
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async {
        let deps = CliDeps {
            store: Arc::new(WindowsSecretStore::new(Environment::Test)),
            launcher: Arc::new(ProcessLauncher),
        };
        let mut gui = GuiSession::open(load_window_config(&config).unwrap(), deps.clone()).unwrap();
        let reply = gui
            .knowledge(KnowledgeRequest::Read { record_id: id })
            .await
            .unwrap();
        assert_eq!(
            serde_json::to_value(&reply).unwrap(),
            shot(&strings(&["knowledge", "read", "--record-id", &rid]), 0)
        );
        let KnowledgeReply::Card(card) = reply else {
            panic!("card")
        };
        assert_eq!(card.effective.title.0, "Карточка AI Off");
        let edit = KnowledgeRequest::Edit(CardEdit {
            operation_id: KnowledgeId(Uuid::new_v4()),
            record_id: id,
            expected_record_revision: card.record_revision,
            expected_catalog_revision: SqlRevision(2),
            expected_category_revision: None,
            patch: CardPatch {
                description: FieldPatch::Set {
                    value: CardDescription(String::new()),
                },
                category_id: FieldPatch::Set { value: None },
                ..CardPatch::default()
            },
        });
        let edited = gui.knowledge(edit.clone()).await.unwrap();
        assert_eq!(
            serde_json::to_value(&edited).unwrap(),
            serde_json::to_value(gui.knowledge(edit).await.unwrap()).unwrap()
        );
        let current = gui
            .knowledge(KnowledgeRequest::Read { record_id: id })
            .await
            .unwrap();
        let KnowledgeReply::Card(current) = current else {
            panic!("card")
        };
        assert_eq!(current.effective.description_source, FieldSource::User);
        assert_eq!(current.effective.category_source, FieldSource::User);
        drop(gui);
    });
    shot(&strings(&["host", "stop", "--json"]), 0);
    owned.stop_wait();
    launch(&mut owned);
    let reopened = shot(&strings(&["knowledge", "read", "--record-id", &rid]), 0);
    assert_eq!(reopened["value"]["effective"]["title"], "Карточка AI Off");
    assert_eq!(reopened["value"]["effective"]["description"], "");
    assert!(reopened["value"]["effective"]["category_id"].is_null());
    assert_eq!(reopened["value"]["projection_state"], "handler_unavailable");
    assert_eq!(
        metadata,
        shot(&strings(&["metadata", "--record-id", &rid]), 0)
    );
    let exported = shot(
        &strings(&["export", "--record-id", &rid, "--format", "png"]),
        0,
    );
    assert!(exported["width"].as_u64().unwrap() > 0);
    assert_eq!(
        status["bridge_state"],
        shot(&strings(&["status", "--no-start", "--json"]), 0)["bridge_state"]
    );
    let bytes = fs::read(
        root.join("assets")
            .read_dir()
            .unwrap()
            .next()
            .unwrap()
            .unwrap()
            .path()
            .join(&rid)
            .join("original.webp"),
    )
    .unwrap();
    assert_eq!(format!("{:x}", Sha256::digest(bytes)), canonical_sha);
    shot(&strings(&["host", "stop", "--json"]), 0);
    owned.stop_wait();
    println!("actual default manual Saved/CLI/GuiSession/replay/conflict/reopen/export PASS, AI Off bridge stopped");
}
