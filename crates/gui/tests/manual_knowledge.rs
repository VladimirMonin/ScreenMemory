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
    shot: PathBuf,
    config_hash: Option<String>,
    failed_stop: bool,
    cleaned: bool,
}
impl Owned {
    fn stop_wait(&mut self) -> Result<(), String> {
        if let Some(mut child) = self.host.take() {
            let until = Instant::now() + Duration::from_secs(15);
            loop {
                if let Some(status) = child.try_wait().map_err(|_| "owned host wait failure")? {
                    println!("owned host process raw_exit={:?}", status.code());
                    if let Some(evidence) = EVIDENCE.get() {
                        fs::write(evidence.join(format!("host-exit-{}.json",Uuid::new_v4())),serde_json::to_vec(&serde_json::json!({"owned_pid":child.id(),"raw_exit":status.code(),"forced":false})).unwrap()).unwrap();
                    }
                    if status.code() != Some(0) {
                        self.failed_stop = true;
                        return Err("owned host nonzero stop; root/credential retained".into());
                    }
                    break;
                }
                if Instant::now() >= until {
                    let _ = child.kill();
                    let status = child.wait().map_err(|_| "owned host reap failure")?;
                    println!(
                        "owned host forced cleanup raw_exit={:?}, clean stop NOT_PROVEN",
                        status.code()
                    );
                    self.failed_stop = true;
                    return Err(
                        "owned host forced cleanup, not clean PASS; root/credential retained"
                            .into(),
                    );
                }
                std::thread::sleep(Duration::from_millis(20));
            }
        }
        Ok(())
    }
    fn cleanup(&mut self) -> Result<(), String> {
        if self.cleaned {
            return Ok(());
        }
        if self.failed_stop {
            return Err("owned stop uncertain; retained root/credential".into());
        }
        if self.host.is_some() {
            let config = self.root.join("root/config/client.json");
            let bytes = fs::read(&config).map_err(|_| "owned pinned config unavailable")?;
            if self.config_hash.as_deref() != Some(&format!("{:x}", Sha256::digest(bytes))) {
                return Err("owned config mutated; cleanup refused".into());
            }
            let out = Command::new(&self.shot)
                .arg("--config")
                .arg(config)
                .args(["host", "stop", "--json"])
                .output()
                .map_err(|_| "owned stop spawn failure")?;
            println!("own cleanup host stop raw_exit={:?}", out.status.code());
            if let Some(evidence) = EVIDENCE.get() {
                let _ = fs::write(evidence.join("cleanup-stop.stdout"), &out.stdout);
                let _ = fs::write(evidence.join("cleanup-stop.stderr"), &out.stderr);
            }
            if !out.status.success() {
                return Err("owned authenticated stop failed; retained root/credential".into());
            }
            self.stop_wait()?;
        }
        if let Some(key) = self.key.as_ref() {
            let vault = WindowsSecretStore::new(Environment::Test);
            if vault.read(key).is_ok() {
                let system = std::env::var_os("SystemRoot").ok_or("native cleanup prerequisite")?;
                let status = Command::new(PathBuf::from(system).join("System32/cmdkey.exe"))
                    .arg(format!("/delete:{}", key.target()))
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .status()
                    .map_err(|_| "exact own credential cleanup failure")?;
                if !status.success() {
                    return Err("exact own credential delete refused".into());
                }
            }
            if !matches!(vault.read(key), Err(StoreError::Missing)) {
                return Err("exact own credential absence not confirmed".into());
            }
            println!("own exact owner credential absence confirmed");
        }
        self.key = None;
        fs::remove_dir_all(&self.root).map_err(|_| "owned root cleanup failure")?;
        fs::remove_dir_all(&self.source_dir).map_err(|_| "owned source cleanup failure")?;
        if self.root.exists() || self.source_dir.exists() {
            return Err("owned root/source absence failure".into());
        }
        self.cleaned = true;
        println!("owned native root/source absence confirmed");
        Ok(())
    }
}
impl Drop for Owned {
    fn drop(&mut self) {
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| self.cleanup()));
        if !matches!(result, Ok(Ok(()))) {
            eprintln!("OWNED_CLEANUP_BLOCKED: exact roots/credential retained; original assertion preserved");
        }
    }
}
#[cfg(feature = "owned-window-tests")]
struct OwnWindow(Child);
#[cfg(feature = "owned-window-tests")]
impl OwnWindow {
    fn id(&self) -> u32 {
        self.0.id()
    }
}
#[cfg(feature = "owned-window-tests")]
impl Drop for OwnWindow {
    fn drop(&mut self) {
        if self.0.try_wait().ok().flatten().is_none() {
            let _ = self.0.kill();
            let _ = self.0.wait();
            eprintln!("OWN_GUI_FORCED_CLEANUP: not graceful PASS");
        }
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
    #[cfg(feature = "owned-window-tests")]
    {
        let check = Command::new(r"C:\Python314\python.exe")
            .arg(repo.join("tests/gui/prerequisites.py"))
            .output()
            .unwrap();
        fs::write(evidence.join("prerequisite.stdout"), &check.stdout).unwrap();
        fs::write(evidence.join("prerequisite.stderr"), &check.stderr).unwrap();
        println!(
            "S06_WINDOW_PREREQUISITE raw_exit={:?}; before fixture side effects",
            check.status.code()
        );
        assert!(
            check.status.success(),
            "required window prerequisite failed, fixture NOT_RUN"
        );
    }
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
        shot: install.join("shot.exe"),
        config_hash: None,
        failed_stop: false,
        cleaned: false,
    };
    let boot = json(
        &install.join("shotd.exe"),
        None,
        &strings(&["--root", root.to_str().unwrap(), "--init"]),
        0,
    );
    let library = LibraryId::new(Uuid::parse_str(boot["library_id"].as_str().unwrap()).unwrap());
    owned.key = Some(CredentialKey::owner(CredentialEnvironment::Test, library));
    assert_eq!(boot["schema_version"], 8);
    println!("owned library UUID={}", library.as_uuid());
    let config = root.join("config/client.json");
    fs::write(&config,serde_json::to_vec(&serde_json::json!({"schema_version":1,"environment":"test","root":root,"library_id":library.as_uuid(),"host_executable":install.join("shotd.exe")})).unwrap()).unwrap();
    owned.config_hash = Some(format!("{:x}", Sha256::digest(fs::read(&config).unwrap())));
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
    let second = shot(
        &strings(&[
            "import",
            "--path",
            source.to_str().unwrap(),
            "--duplicates",
            "keep_both",
            "--wait-ms",
            "30000",
            "--json",
        ]),
        0,
    );
    assert_eq!(second["items"].as_array().unwrap().len(), 1);
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
    assert_eq!(
        shot(&strings(&["knowledge", "read", "--record-id", &rid]), 0)["value"]["projection_state"],
        "current"
    );
    let search_id = Uuid::new_v4().to_string();
    let browse = shot(
        &strings(&["search", "--search-id", &search_id, "--limit", "1"]),
        0,
    );
    let cursor = browse["value"]["next_cursor"].as_str().unwrap().to_owned();
    assert_eq!(
        shot(
            &strings(&["search", "page", "--cursor", &Uuid::new_v4().to_string()]),
            5
        )["code"],
        "SEARCH_EXPIRED"
    );
    assert_eq!(
        shot(&strings(&["search", "--text", "***"]), 2)["code"],
        "NO_LEXICAL_TERMS"
    );
    assert_eq!(
        shot(
            &strings(&[
                "search",
                "--text",
                "x",
                "--filters-json",
                r#"{"application":{"kind":"unattributed"},"has_ocr":null,"has_applied_ai":null,"source_kind":null}"#
            ]),
            2
        )["code"],
        "SEARCH_FILTER_UNSUPPORTED"
    );
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
    assert_eq!(
        shot(&strings(&["search", "page", "--cursor", &cursor]), 5)["code"],
        "SEARCH_SNAPSHOT_CHANGED"
    );
    let search = shot(
        &strings(&["search", "--text", "Карточка", "--limit", "30"]),
        0,
    );
    assert_eq!(search["value"]["items"][0]["record_id"], rid);
    assert_eq!(
        search["value"]["items"][0]["category_label"],
        "Ручная категория"
    );
    assert_eq!(
        search["value"]["items"][0]["excerpt"],
        "Сохранено без модели"
    );
    #[cfg(feature = "owned-window-tests")]
    {
        load_window_config(&config).unwrap(); // Existing trusted tuple validation, no authority from DOM.
        let config_hash = format!("{:x}", Sha256::digest(fs::read(&config).unwrap()));
        let image_hash = format!(
            "{:x}",
            Sha256::digest(fs::read(install.join("screenmemory-gui.exe")).unwrap())
        );
        let listener = std::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)).unwrap();
        let port = listener.local_addr().unwrap().port();
        drop(listener);
        let window = Command::new(install.join("screenmemory-gui.exe"))
            .env(
                "WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS",
                format!("--remote-debugging-port={port} --remote-debugging-address=127.0.0.1"),
            )
            .arg("--config")
            .arg(&config)
            .stdout(Stdio::null())
            .stderr(Stdio::from(
                fs::File::create(evidence.join("window.stderr")).unwrap(),
            ))
            .spawn()
            .unwrap();
        let mut window = OwnWindow(window);
        assert!(window.0.try_wait().unwrap().is_none());
        let identity=Command::new("powershell.exe").args(["-NoProfile","-NonInteractive","-Command"])
        .arg(format!("$p=Get-CimInstance Win32_Process -Filter 'ProcessId={}'; if(!$p){{exit 2}};[Console]::Write($p.CreationDate.ToUniversalTime().Ticks)",window.id())).output().unwrap();
        assert!(identity.status.success());
        let created = String::from_utf8(identity.stdout).unwrap();
        let _: i64 = created.trim().parse().unwrap();
        let fixture = include_bytes!("../../../tests/fixtures/images/odd-alpha.png");
        let width = u32::from_be_bytes(fixture[16..20].try_into().unwrap());
        let height = u32::from_be_bytes(fixture[20..24].try_into().unwrap());
        let dom = Command::new(r"C:\Python314\python.exe")
            .arg(repo.join("tests/gui/inspect_search.py"))
            .arg("--config")
            .arg(&config)
            .arg("--pid")
            .arg(window.id().to_string())
            .arg("--port")
            .arg(port.to_string())
            .arg("--output")
            .arg(&evidence)
            .arg("--width")
            .arg(width.to_string())
            .arg("--height")
            .arg(height.to_string())
            .arg("--library-id")
            .arg(library.as_uuid().to_string())
            .arg("--created-ticks")
            .arg(created.trim())
            .arg("--config-sha256")
            .arg(&config_hash)
            .arg("--image-sha256")
            .arg(image_hash)
            .arg("--canonical-sha256")
            .arg(format!("{:x}", Sha256::digest(&original)))
            .arg("--record-id")
            .arg(&rid)
            .output();
        let shot_window = Command::new("powershell.exe")
            .args(["-NoProfile", "-NonInteractive", "-File"])
            .arg(repo.join("tests/gui/capture_search.ps1"))
            .arg("-OwnedPid")
            .arg(window.id().to_string())
            .arg("-Config")
            .arg(&config)
            .arg("-Output")
            .arg(evidence.join("search-window.png"))
            .output()
            .unwrap();
        fs::write(evidence.join("window-capture.stdout"), &shot_window.stdout).unwrap();
        fs::write(evidence.join("window-capture.stderr"), &shot_window.stderr).unwrap();
        fs::write(evidence.join("window-capture.json"),serde_json::to_vec(&serde_json::json!({"raw_exit":shot_window.status.code(),"owned_pid":window.id(),"genuine_input":"NOT_PROVEN"})).unwrap()).unwrap();
        let close = Command::new("powershell.exe")
            .args(["-NoProfile", "-NonInteractive", "-File"])
            .arg(repo.join("tests/gui/close_search.ps1"))
            .arg("-OwnedPid")
            .arg(window.0.id().to_string())
            .arg("-CreatedTicks")
            .arg(created.trim())
            .arg("-Config")
            .arg(&config)
            .arg("-ConfigSha256")
            .arg(&config_hash)
            .output()
            .unwrap();
        fs::write(evidence.join("window-close.stdout"), &close.stdout).unwrap();
        fs::write(evidence.join("window-close.stderr"), &close.stderr).unwrap();
        assert!(close.status.success(), "own window close refused");
        let deadline = Instant::now() + Duration::from_secs(12);
        loop {
            if let Some(status) = window.0.try_wait().unwrap() {
                println!("own GUI clean raw_exit={:?}", status.code());
                assert_eq!(status.code(), Some(0));
                break;
            }
            assert!(Instant::now() < deadline, "own GUI close timeout");
            std::thread::sleep(Duration::from_millis(20));
        }
        let dom = dom.unwrap();
        fs::write(evidence.join("dom.stdout"), &dom.stdout).unwrap();
        fs::write(evidence.join("dom.stderr"), &dom.stderr).unwrap();
        println!(
            "owned GUI DOM raw_exit={:?}; genuine_input NOT_PROVEN",
            dom.status.code()
        );
        assert!(
            dom.status.success(),
            "owned GUI DOM query/preview check failed"
        );
        assert!(
            shot_window.status.success(),
            "owned window PrintWindow capture failed"
        );
    }
    #[cfg(not(feature = "owned-window-tests"))]
    println!("S06_OWN_WINDOW=NOT_RUN; required separate --features owned-window-tests gate");
    assert_eq!(result, shot(&args, 0));
    let mut changed = args.clone();
    *changed.last_mut().unwrap() = "changed parameters".into();
    assert_eq!(shot(&changed, 5)["code"], "IDEMPOTENCY_CONFLICT");
    let mut stale = args.clone();
    stale[5] = Uuid::new_v4().to_string();
    assert_eq!(shot(&stale, 5)["code"], "REVISION_CONFLICT");
    let rename = CategoryMutation {
        operation_id: KnowledgeId(Uuid::new_v4()),
        expected_catalog_revision: SqlRevision(2),
        change: CategoryChange::Rename {
            category_id: KnowledgeId(Uuid::parse_str(cid).unwrap()),
            expected_revision: SqlRevision(1),
            name: CategoryName("Разработка77".into()),
            description: CategoryDescription("synthetic".into()),
        },
    };
    shot(
        &strings(&[
            "knowledge",
            "category",
            "--request-json",
            &serde_json::to_string(&rename).unwrap(),
        ]),
        0,
    );
    assert_eq!(
        shot(&strings(&["search", "--text", "Разработка77"]), 0)["value"]["items"][0]["record_id"],
        rid
    );
    assert!(
        shot(&strings(&["search", "--text", "Ручная"]), 0)["value"]["items"]
            .as_array()
            .unwrap()
            .is_empty()
    );
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
        let query = sm_protocol::search::SearchStart {
            search_id: KnowledgeId(Uuid::new_v4()),
            text: "Карточка".into(),
            filters: sm_protocol::search::SearchFilters::default(),
            limit: 30,
            candidate_depth: 100,
        };
        let search = gui
            .search(sm_protocol::search::SearchRequest::Start(query))
            .await
            .unwrap();
        let sm_protocol::search::SearchReply::Page(page) = search else {
            panic!("page")
        };
        assert_eq!(page.items[0].record_id, id);
        assert_eq!(page.items[0].title, "Карточка AI Off");
        assert_eq!(page.items[0].excerpt, "Сохранено без модели");
        let preview = gui
            .read_asset_full(sm_domain::AssetId::new(page.items[0].asset_id.0))
            .await;
        assert!(preview.safe_code.is_none());
        assert_eq!(
            preview.data_hex,
            original
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect::<String>()
        );
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
            expected_catalog_revision: SqlRevision(3),
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
    let delete = CategoryMutation {
        operation_id: KnowledgeId(Uuid::new_v4()),
        expected_catalog_revision: SqlRevision(3),
        change: CategoryChange::State {
            category_id: KnowledgeId(Uuid::parse_str(cid).unwrap()),
            expected_revision: SqlRevision(2),
            state: CategoryState::Deleted,
            expected_assigned_records: 0,
        },
    };
    shot(
        &strings(&[
            "knowledge",
            "category",
            "--request-json",
            &serde_json::to_string(&delete).unwrap(),
        ]),
        0,
    );
    assert!(
        shot(&strings(&["search", "--text", "Разработка77"]), 0)["value"]["items"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    shot(&strings(&["host", "stop", "--json"]), 0);
    owned.stop_wait().unwrap();
    launch(&mut owned);
    let reopened = shot(&strings(&["knowledge", "read", "--record-id", &rid]), 0);
    assert_eq!(reopened["value"]["effective"]["title"], "Карточка AI Off");
    assert_eq!(reopened["value"]["effective"]["description"], "");
    assert!(reopened["value"]["effective"]["category_id"].is_null());
    assert_eq!(reopened["value"]["projection_state"], "current");
    assert_eq!(
        shot(&strings(&["search", "page", "--cursor", &cursor]), 5)["code"],
        "SEARCH_EXPIRED"
    );
    assert_eq!(
        shot(&strings(&["search", "--text", "Карточка"]), 0)["value"]["items"][0]["record_id"],
        rid
    );
    assert!(
        shot(&strings(&["search", "--text", "Сохранено"]), 0)["value"]["items"]
            .as_array()
            .unwrap()
            .is_empty()
    );
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
    owned.stop_wait().unwrap();
    owned.cleanup().unwrap();
    println!("actual default manual Saved/CLI/GuiSession/replay/conflict/reopen/export PASS, AI Off bridge stopped");
}
