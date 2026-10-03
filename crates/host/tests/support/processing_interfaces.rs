// Production executable/JSON command proof, not a constructor-only bootstrap oracle.
fn installed_command(exe: &Path, node: &Path) -> tokio::process::Command {
    let private = private_node_command(node).unwrap();
    let mut c = tokio::process::Command::new(exe);
    c.env_clear();
    for (key, value) in private.as_std().get_envs() {
        if let Some(value) = value {
            c.env(key, value);
        }
    }
    c.current_dir(exe.parent().unwrap()).kill_on_drop(true);
    c
}
async fn native_json(mut c: tokio::process::Command, expected: i32) -> serde_json::Value {
    let out = tokio::time::timeout(Duration::from_secs(30), c.output())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(out.status.code(), Some(expected), "actual native exit");
    let lines = std::str::from_utf8(&out.stdout)
        .unwrap()
        .lines()
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>();
    assert_eq!(lines.len(), 1, "one JSON stdout");
    serde_json::from_str(lines[0]).unwrap()
}
async fn shot(
    install: &Path,
    node: &Path,
    config: &Path,
    args: &[String],
    expected: i32,
) -> serde_json::Value {
    let mut c = installed_command(&install.join("shot.exe"), node);
    c.arg("--config").arg(config).args(args);
    native_json(c, expected).await
}
fn strings(args: &[&str]) -> Vec<String> {
    args.iter().map(|s| (*s).into()).collect()
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn s04_interfaces_actual_shotd_installation_json_cli_inventory_off_and_self_test() {
    let _single = SDK_OWNER.lock().await;
    let fixture = Fixture::new().await;
    let mut server = Server::new(&fixture).await;
    let install = fixture.package_root.join("installation");
    fs::create_dir(&install).unwrap();
    for exe in ["shotd.exe", "shot.exe", "shot-image-worker.exe"] {
        fs::copy(
            repo().join(".pi/staging-target-t09b/debug").join(exe),
            install.join(exe),
        )
        .unwrap();
    }
    fs::rename(&fixture.package, install.join("sdk-runtime")).unwrap();
    let root = library();
    println!(
        "owned interfaces library {}",
        root.file_name().unwrap().to_string_lossy()
    );
    let mut init = installed_command(&install.join("shotd.exe"), &fixture.node);
    init.arg("--root").arg(&root).arg("--init");
    let boot = native_json(init, 0).await;
    let library_id = LibraryId::new(Uuid::parse_str(boot["library_id"].as_str().unwrap()).unwrap());
    println!("owned interfaces library identity {}", library_id.as_uuid());
    let config = root.join("config/client.json");
    fs::write(&config,serde_json::to_vec(&serde_json::json!({"schema_version":1,"environment":"test","root":root,"library_id":library_id.as_uuid(),"host_executable":install.join("shotd.exe")})).unwrap()).unwrap();
    let never = root.parent().unwrap().join(Uuid::new_v4().to_string());
    assert!(!never.exists());
    let mut stalled = installed_command(&install.join("shotd.exe"), &fixture.node)
        .args(["--root", never.to_str().unwrap(), "--enroll-runtime-token"])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    println!(
        "owned stdin deadline pid {:?} root {}",
        stalled.id(),
        never.file_name().unwrap().to_str().unwrap()
    );
    let held_input = stalled.stdin.take().unwrap();
    let refused = tokio::time::timeout(Duration::from_secs(14), stalled.wait_with_output())
        .await
        .unwrap()
        .unwrap();
    drop(held_input);
    assert_eq!(refused.status.code(), Some(8));
    assert!(
        !never.exists(),
        "no root authority acquired on stdin deadline"
    );
    assert_eq!(server.stats().await["loads"], 0);
    // Exact vault enrollment via bounded nonterminal stdin, never argv or local plaintext settings.
    let mut enrollment = installed_command(&install.join("shotd.exe"), &fixture.node);
    enrollment
        .arg("--root")
        .arg(&root)
        .arg("--enroll-runtime-token")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    let mut child = enrollment.spawn().unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(b"sk-lm-syntest1:01234567890123456789")
        .await
        .unwrap();
    let out = tokio::time::timeout(Duration::from_secs(30), child.wait_with_output())
        .await
        .unwrap()
        .unwrap();
    assert!(out.status.success());
    assert!(!String::from_utf8_lossy(&out.stdout).contains("syntest"));
    assert!(!String::from_utf8_lossy(&out.stderr).contains("syntest"));
    let runtime_config = root.join("config/processing-runtime.json");
    let port = server
        .url
        .rsplit(':')
        .next()
        .unwrap()
        .parse::<u16>()
        .unwrap();
    let trusted_config = serde_json::json!({"schema_version":1,"library_id":library_id.as_uuid(),"port":port,"authentication":"credential","ask_load_approved":false,"image_side":1024,"model_autostart":false});
    for (field, value) in [
        ("model_autostart", serde_json::json!(true)),
        ("library_id", serde_json::json!(Uuid::new_v4())),
        ("package_path", serde_json::json!("injected")),
        ("port", serde_json::json!(0)),
    ] {
        let mut malformed = trusted_config.clone();
        malformed[field] = value;
        fs::write(&runtime_config, serde_json::to_vec(&malformed).unwrap()).unwrap();
        let out = installed_command(&install.join("shotd.exe"), &fixture.node)
            .arg("--root")
            .arg(&root)
            .output()
            .await
            .unwrap();
        assert_eq!(
            out.status.code(),
            Some(4),
            "host config refusal before runtime dispatch"
        );
        assert!(out.stdout.is_empty());
    }
    assert_eq!(server.stats().await["loads"], 0);
    fs::write(
        &runtime_config,
        serde_json::to_vec(&trusted_config).unwrap(),
    )
    .unwrap();
    let mut host = installed_command(&install.join("shotd.exe"), &fixture.node)
        .arg("--root")
        .arg(&root)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .unwrap();
    println!("owned interfaces shotd pid {}", host.id().unwrap());
    let status = tokio::time::timeout(Duration::from_secs(15), async {
        loop {
            let mut c = installed_command(&install.join("shot.exe"), &fixture.node);
            c.arg("--config")
                .arg(&config)
                .args(["status", "--no-start", "--json"]);
            let output = c.output().await.unwrap();
            if output.status.success() {
                break serde_json::from_slice::<serde_json::Value>(&output.stdout).unwrap();
            }
            tokio::time::sleep(Duration::from_millis(30)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(status["ai_enabled"], false);
    assert_eq!(status["bridge_state"], "stopped");
    let initial = shot(
        &install,
        &fixture.node,
        &config,
        &strings(&["processing", "status"]),
        0,
    )
    .await;
    assert_eq!(initial["value"]["runtime_configured"], true);
    assert_eq!(initial["value"]["policy"]["value"]["mode"], "off");
    assert_eq!(server.stats().await["loads"], 0);
    // Explicit inventory works while Off; no model dispatch/adoption.
    let inv = shot(
        &install,
        &fixture.node,
        &config,
        &strings(&["processing", "inventory"]),
        0,
    )
    .await;
    assert_eq!(
        inv["value"]["downloaded"][0]["model_key"],
        "synthetic/vision.gguf"
    );
    assert_eq!(server.stats().await["loads"], 0);
    let source = fixture.package_root.join("synthetic.png");
    fs::write(
        &source,
        include_bytes!("../../../../tests/fixtures/images/odd-alpha.png"),
    )
    .unwrap();
    let saved = shot(
        &install,
        &fixture.node,
        &config,
        &strings(&[
            "import",
            "--path",
            source.to_str().unwrap(),
            "--wait-ms",
            "30000",
            "--json",
        ]),
        0,
    )
    .await;
    let record = saved["items"][0]["receipt"]["record_id"]
        .as_str()
        .unwrap()
        .to_owned();
    fs::remove_file(source).unwrap();
    let mut p = profile();
    for reasoning in [ReasoningMode::Off, ReasoningMode::Budget100] {
        p.reasoning = reasoning;
        let refused = shot(
            &install,
            &fixture.node,
            &config,
            &strings(&[
                "processing",
                "save-profile",
                "--revision",
                "0",
                "--profile-json",
                &serde_json::to_string(&p).unwrap(),
            ]),
            4,
        )
        .await;
        assert_eq!(refused["code"], "MODEL_CONFIG_UNSUPPORTED");
    }
    p.reasoning = ReasoningMode::BackendDefault;
    let profile_reply = shot(
        &install,
        &fixture.node,
        &config,
        &strings(&[
            "processing",
            "save-profile",
            "--revision",
            "0",
            "--profile-json",
            &serde_json::to_string(&p).unwrap(),
        ]),
        0,
    )
    .await;
    assert_eq!(profile_reply["value"]["revision"], "1");
    let policy = shot(
        &install,
        &fixture.node,
        &config,
        &strings(&[
            "processing",
            "policy",
            "--mode",
            "manual",
            "--query",
            "allow",
        ]),
        0,
    )
    .await;
    let rev = policy["value"]["revision"].as_str().unwrap();
    let op = Uuid::new_v4().to_string();
    let args = strings(&[
        "processing",
        "self-test",
        "--record-id",
        &record,
        "--profile-id",
        &p.id.as_uuid().to_string(),
        "--policy-revision",
        rev,
        "--operation-id",
        &op,
    ]);
    let batch = shot(&install, &fixture.node, &config, &args, 0).await;
    let replay = shot(&install, &fixture.node, &config, &args, 0).await;
    assert_eq!(replay, batch);
    let batch_id = batch["value"]["id"].as_str().unwrap();
    let done = tokio::time::timeout(Duration::from_secs(30), async {
        loop {
            let b = shot(
                &install,
                &fixture.node,
                &config,
                &strings(&["processing", "batch", "--id", batch_id]),
                0,
            )
            .await;
            if b["value"]["jobs"][0]["state"] == "succeeded" {
                break b;
            }
            assert_ne!(b["value"]["jobs"][0]["state"], "failed");
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(done["value"]["jobs"][0]["probe_result"]["validated"], true);
    let analyze = shot(
        &install,
        &fixture.node,
        &config,
        &strings(&[
            "processing",
            "analyze",
            "--record-id",
            &record,
            "--policy-revision",
            rev,
            "--operation-id",
            &Uuid::new_v4().to_string(),
        ]),
        0,
    )
    .await;
    assert_eq!(analyze["value"]["jobs"][0]["reason"], "handler_unavailable");
    let analyzed_batch = analyze["value"]["id"].as_str().unwrap();
    let mut batch_revision = analyze["value"]["revision"].as_str().unwrap().to_owned();
    for action in ["pause", "resume"] {
        let control = shot(
            &install,
            &fixture.node,
            &config,
            &strings(&[
                "processing",
                action,
                "--id",
                analyzed_batch,
                "--revision",
                &batch_revision,
            ]),
            0,
        )
        .await;
        assert_eq!(control["value"]["control"], action);
        batch_revision = control["value"]["revision"].as_str().unwrap().to_owned();
    }
    let analyzed_job = analyze["value"]["jobs"][0]["id"].as_str().unwrap();
    let cancel = shot(
        &install,
        &fixture.node,
        &config,
        &strings(&[
            "processing",
            "cancel",
            "--id",
            analyzed_job,
            "--revision",
            "1",
        ]),
        0,
    )
    .await;
    assert_eq!(cancel["value"]["state"], "cancelled");
    let retry_revision = cancel["value"]["revision"].as_str().unwrap();
    let retry = shot(
        &install,
        &fixture.node,
        &config,
        &strings(&[
            "processing",
            "retry",
            "--id",
            analyzed_job,
            "--revision",
            retry_revision,
        ]),
        0,
    )
    .await;
    assert_eq!(retry["value"]["reason"], "handler_unavailable");
    let stale = shot(
        &install,
        &fixture.node,
        &config,
        &strings(&[
            "processing",
            "retry",
            "--id",
            analyzed_job,
            "--revision",
            retry_revision,
        ]),
        5,
    )
    .await;
    assert_eq!(stale["code"], "REVISION_CONFLICT");
    let listed = shot(
        &install,
        &fixture.node,
        &config,
        &strings(&["processing", "jobs"]),
        0,
    )
    .await;
    assert_eq!(listed["value"]["jobs"].as_array().unwrap().len(), 2);
    assert_eq!(listed["value"]["bridge_state"], "ready");
    let cancelled_batch = shot(
        &install,
        &fixture.node,
        &config,
        &strings(&[
            "processing",
            "cancel-batch",
            "--id",
            analyzed_batch,
            "--revision",
            &batch_revision,
        ]),
        0,
    )
    .await;
    assert_eq!(cancelled_batch["value"]["control"], "cancel");
    // Real finite S05-blocked plans prove list keyset bounds without synthetic SQL inserts.
    for _ in 0..33 {
        let b = shot(
            &install,
            &fixture.node,
            &config,
            &strings(&[
                "processing",
                "analyze",
                "--record-id",
                &record,
                "--policy-revision",
                rev,
                "--operation-id",
                &Uuid::new_v4().to_string(),
            ]),
            0,
        )
        .await;
        assert_eq!(b["value"]["jobs"][0]["reason"], "handler_unavailable");
    }
    let first = shot(
        &install,
        &fixture.node,
        &config,
        &strings(&["processing", "jobs"]),
        0,
    )
    .await;
    assert_eq!(first["value"]["jobs"].as_array().unwrap().len(), 32);
    let before = first["value"]["next_before"].as_str().unwrap();
    let second = shot(
        &install,
        &fixture.node,
        &config,
        &strings(&["processing", "jobs", "--before", before]),
        0,
    )
    .await;
    assert_eq!(second["value"]["jobs"].as_array().unwrap().len(), 3);
    assert!(second["value"]["next_before"].is_null());
    let ids = first["value"]["jobs"]
        .as_array()
        .unwrap()
        .iter()
        .chain(second["value"]["jobs"].as_array().unwrap())
        .map(|j| j["job"]["id"].as_str().unwrap())
        .collect::<std::collections::HashSet<_>>();
    assert_eq!(ids.len(), 35);
    let job = done["value"]["jobs"][0]["id"].as_str().unwrap();
    let ledger = shot(
        &install,
        &fixture.node,
        &config,
        &strings(&["processing", "execution", "--id", job]),
        0,
    )
    .await;
    let load = ledger["value"]["calls"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["kind"] == "load")
        .unwrap();
    assert_eq!(load["load_configuration"], "reported_applied");
    assert_eq!(load["load_requested"]["context_length"], 8192);
    assert_eq!(load["load_reported_applied"]["context_length"], 8192);
    assert_eq!(load["native_ttl_requested_seconds"], 900);
    assert_eq!(load["native_ttl_verification"], "not_verified");
    let _ = shot(
        &install,
        &fixture.node,
        &config,
        &strings(&["processing", "policy", "--mode", "off"]),
        0,
    )
    .await;
    let stats = server.stats().await;
    assert_eq!(stats["loads"], 1);
    assert_eq!(stats["predictions"], 1);
    println!("interfaces actual native checks passed: Off/load0 inventory/load0 own Saved selftest/load1 prediction1 exact replay S05 blocked");
    let _ = shot(
        &install,
        &fixture.node,
        &config,
        &strings(&["host", "stop", "--json"]),
        0,
    )
    .await;
    assert!(tokio::time::timeout(Duration::from_secs(15), host.wait())
        .await
        .unwrap()
        .unwrap()
        .success());
    server.close().await;
    let vault = sm_credentials::WindowsSecretStore::new(sm_isolation::Environment::Test);
    let env = private_node_command(&fixture.node).unwrap();
    let system = env
        .as_std()
        .get_envs()
        .find(|(k, _)| k.to_string_lossy() == "SystemRoot")
        .unwrap()
        .1
        .unwrap()
        .to_owned();
    for key in [
        CredentialKey::owner(CredentialEnvironment::Test, library_id),
        CredentialKey::runtime_backend(CredentialEnvironment::Test, library_id),
    ] {
        assert!(vault.read(&key).is_ok());
        let out = tokio::process::Command::new(PathBuf::from(&system).join("System32/cmdkey.exe"))
            .arg(format!("/delete:{}", key.target()))
            .output()
            .await
            .unwrap();
        assert!(out.status.success());
        assert!(matches!(vault.read(&key), Err(StoreError::Missing)));
        println!("own exact credential absence confirmed {}", key.target());
    }
    remove_own(root);
    remove_own(fixture.package_root);
}
