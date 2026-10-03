# S04 — полезные GUI / JSON CLI, checked interface slice

Parent: **ACCEPT_CODE_SCOPED**, fresh review и targeted follow-up pass/findings[].
Итог/границы: [result](result.md), не whole S04/release. Ниже сохранён writer handoff
на base `c0648dca`; source был released без staging/commit у writer.
Прежние [SDK](sdk-runtime.md) и [execution](processing.md) receipts не переписаны.
FTS5-only / ADR-011 сохранён; embeddings и S05 knowledge не добавлены.

## Что действительно подключено

- Настоящий `shotd` вызывает `bootstrap_from_installation`: после root/HostLease/library
  identity, до listener читает фиксированный `config/processing-runtime.json` (≤4096 bytes,
  без reparse/unknown fields). SDK/Node — только trusted sibling `sdk-runtime` текущего
  host image; image worker — существующий sibling. Нет ENV/PATH/CWD/wire package authority.
- Конфиг задаёт schema1, exact library UUID, numeric loopback port, explicit
  `authentication: credential|guest`, `ask_load_approved`, image_side1024|2048,
  `model_autostart:false`. Отсутствие файла оставляет runtime unconfigured; неверная
  identity, port0, autostarttrue и injected package_path отказывают до SDK dispatch.
- `shotd --root <own-root> --enroll-runtime-token`: nonterminal stdin UTF-8 1…2048 bytes,
  без newline/control, EOF ≤10s. Timeout exit8 до root/vault authority. Под HostLease
  create-new exact `<namespace>/library/<UUID>/lm-studio` target; нет overwrite/fallback.
  Backend profile имеет пустые application scopes; shared client отказывает ему до
  vault read. Guest — только backend policy, не обход authenticated application session.
  Token не передаётся argv/ENV/WebView/log/localStorage.
- `ProcessingRequest::Overview` отдаёт policy/latest profiles≤32/principal jobs≤32 с
  keyset cursor, revisions, batch state, observed bridge/capabilities. Explicit Inventory
  проходит Owner ReadRuntimeInventory через actor channel8: может запустить private
  bridge при Off, но не load/adopt модель. Overview/mount/profile save этого не делают.
- Shared typed Rust client используется Tauri handler `gui_processing` и настоящим
  JSON CLI `shot processing status|jobs|inventory|policy|profile|save-profile|self-test|
  background-self-test|analyze|batch|pause|resume|cancel-batch|cancel|retry|execution|reconcile`.
  Stable operation/profile/record IDs и expected revisions обязательны; readonly reconcile
  не разрешает replay/eviction. Unsupported authority flags/actions отказывают.
- Реальный React processing panel: Automatic/Manual/Off, независимые background/query
  permissions, profile/load configuration, explicit inventory/model-key selection,
  capabilities, queue/control и TechnicalOwnerSelfTest. Перед invoke точный план сохраняется
  под library-scoped localStorage key (только IDs/revision); explicit replay восстанавливает
  тот же operation ID после lost response/reopen. Никакого automatic replay/load effect.
- Requested / SDK-reported-applied load configuration и status типизированы и доступны
  в execution ledger. Physical unload всегда Unsupported; host idle/AfterBatch/KeepLoaded
  Unsupported, NoHostEviction не retention guarantee. TTL requested900 / not_verified
  не доказательство eviction. Off/Budget100 reasoning отвергаются до dispatch;
  BackendDefault выбран явно, не эмулируется.
- Library/import/canonical/preview/export независимы от AI. Analyze создаёт настоящий
  `blocked:handler_unavailable`, не AI-карточку. Search/knowledge_pipeline не реализованы.

## Проверки и raw evidence

Пути в таблице относительны `.pi/temporary/`. Rust1.98.1 через существующий
`cmd.exe /d /c .pi\\t09b-run.cmd <cargo args>`; Node24.13.0 — pinned `.pi/toolchains`.
Команды offline/locked; installs/downloads/model weights не выполнялись.

| UUID / raw logs | Реальная команда / outcome |
|---|---|
| `f239e72e-70d4-40b8-af1b-ce004eed7bcd/default-actual-host-cli.log` | **Default features**, `cargo test --offline --locked -p sm-host --test processing_execution s04_interfaces_actual -- --test-threads=1 --nocapture`, raw0,1/1,27.99s. Actual copied shotd/shot/image worker + staged published SDK/private Node outside developer PATH + own WS fixture + exact native vault. Held-open stdin timeout exit8/root absent; configured Off/load0; inventory/load0; Saved/source deletion; reasoning Off/Budget100 refusal; Manual/query Allow/profile; selftest load1/prediction1; stable operation replay; S05 blocked; actual pause/resume/cancel/retry/stale exit5;35 public plans paginate32+3 unique jobs; typed requested/applied context8192 and TTL900/not_verified. Host/endpoint stop, exact owner/backend vault absence, exact library/package roots removed. |
| `f239e72e-70d4-40b8-af1b-ce004eed7bcd/{clippy-default,clippy-feature,default-build,boundaries,fmt}.log` | Affected8 default `cargo clippy --offline --locked ... --all-targets -- -D warnings` raw0; host processing-test-seams all-target clippy raw0; `cargo build --offline --locked -p sm-host -p sm-cli -p sm-gui` raw0 (restored default binaries); Python exact dependency/boundary contract raw0; changed main/native fixture rustfmt raw0. |
| `e07410c0-2933-45fc-8bd7-698288416b9f/cli-tests.log` | `cargo test --offline --locked -p sm-cli --test cli_surface --test processing_args -- --nocapture`, raw0,6+2 tests. Schema6 oracle, closed native vocabulary/stable IDs/authority refusal/revision-conflict5; known own UUID roots logged. |
| `4e8d05a2-96a8-43c0-a0ba-eb0d600f860b/gui-surface.log` | `cargo test --offline --locked -p sm-gui --test gui_surface -- --nocapture`, raw0,12/12. Actual composed shared-session GUI/CLI policy/profile/status/error parity; backend-profile authentication refusal, stale revision leaves host ready. |
| `97f87313-1785-4970-ab31-b65983c1414a/host-config-tests.log` | `cargo test --offline --locked -p sm-host --lib runtime_config`, raw0,2/2: closed trusted config and bounded UTF-8 secret input negatives. Main deadline separately proved by final actual executable test. |
| `4e8d05a2-96a8-43c0-a0ba-eb0d600f860b/` | GUI TS types/boundaries/parity/build raw0; `node apps/gui/node_modules/vitest/vitest.mjs run --root apps/gui`, raw0,19/19 including3 panel tests: StrictMode render performs zero invokes, truthful unsupported/blocked labels, no private runtime facade. Static render tests are not native button/input proof. |
| `e5f52978-d05c-4aae-8b2d-d9a503bc7a0e/protocol-tests.log` | `cargo test --offline --locked -p sm-protocol s04`, raw0,shared processing positive/negative fixture test1/1. |
| `9d3810c5-1c8d-4e13-ae35-2b52b84a0dcb/` | Protocol TS `tsc --noEmit`, Rust schema generator `--check schemas`, TS generator `--check`, raw0 each; `e07410c0…/schemas.log`: Python JSON-schema validation raw0. |
| `576a7a9b-b656-45a2-9eb9-97322cfdca5d/protocol-fixtures.log` | `node --test packages/protocol/tests/protocol.test.mjs`, raw0,8/8 incl shared S04 + real strict calendar/SafeMessage mutations. Unsigned uint64 format explicitly registered in both mutation helpers; strict validation/coercion rules not weakened. |

### Реальный own Tauri visual / lifecycle

- Synthetic stand receipt `9d7e1245-4e95-4e69-b8a3-dfe1d0f0d7d3/`: собственный
  `numbered-grid.png` (3×2 pixels) импортирован настоящим CLI, SavedNew1; source удалён.
  `native-import.log`, `native-processing-status.log`, `native-export.log` (PNG export raw0).
  Это не визуальный proof большой галереи и не physical model/capture proof.
- Существующий `scripts/capture-preview-window.ps1 -GuiProcessId <owned PID>
  -PreviewRoot <exact owned preview> -OutputRoot <fresh UUID>` raw0. Exact PID/image/config
  validated; **PrintWindow только own HWND**, physical PM-aware1462×1136, DPI144,
  desktop_capture=false. `421e30f4-6c34-4cde-94c9-38d9d2d160da/owned-window.png`,
  SHA256 `8e608bc8d0437820be3411f672211850655620f85628c282d9885f0b0073b241`.
  Агент прочитал изображение: S04 pane реально отрисован, readable Off/bridge stopped,
  autostartfalse, separate load selectors, pause explanation, Unsupported unload/retention,
  closed advanced profile section, explicit technical self-test heading; горизонтальный
  overflow не наблюдался. Advanced controls ниже/внутри section визуально не доказаны.
- Own UIA summary lookup `profile-uia.log` raw1/NOT_PROVEN: Invoke/input не доказан,
  genuine input не заявляется. При поздней guarded финализации прежний GUI PID отсутствовал:
  `finalize.log` raw1 и `gui-process-identity.json`; причина/exit UNKNOWN, чужое не тронуто.
- **Один необходимый existing bounded window/lifecycle gate** после стабильного GUI:
  `powershell.exe -NoProfile -NonInteractive -File scripts/preview-gui.ps1 -Action window
  -SkipBuild -PreviewRoot <same exact owned preview> -WindowHoldSeconds 2 -Cleanup
  -OutputRoot <fresh UUID>`, raw0. `f7dc156a-0a6a-47d7-9a67-ab25db2a4e6e/result.json`:
  self-check0 → own window close0 → same host outlives GUI → reopened client same
  instance0 → host stop0/endpoint gone → exact root/owner credential removal0/readback1168.
  Его короткий capture содержит пустую поверхность: это **не rendered UI PASS**;
  rendered settings proof — только предыдущий полностью прочитанный PrintWindow.
  Main stdin-only deadline добавлен позже; default non-enrollment bootstrap дополнительно
  исполнен финальным actual native CLI/SDK case, GUI source после screenshot не менялся.

## Исправленные FAIL сохранены, не переобъявлены PASS

- `79507d82-1cbf-426b-b6bd-0d8376a5e97c/interfaces.log`: raw101 только неверная oracle
  `STOPPED` вместо `stopped`. `cleanup-new-failure.log` raw0: записанные children отсутствуют,
  exact owner/backend vault1168 и две own UUID roots удалены.
- `4e8d05a2…/cli-surface.log`: raw101 stale schema5 после additive6. Oracle исправлена,
  следующие roots имеют UUIDv4+identity log; текущие6 tests raw0. Absence старого
  non-UUID/lacking-receipt CLI fixture не доказан — никакого guessed cleanup.
- `e07410c0…/boundaries.log`: raw1 obsolete exact frontend set; добавлены только два
  явных panel/test файла. Следующий boundary raw0, ни одного blanket exception.
- `9d3810c5…/protocol-fixtures.log`: raw1,6/8: оба existing mutation validators не знали
  uint64. Исправлены explicit numeric guards, последующий full protocol8/8 raw0.
- `34576c86-69dd-42af-976d-7f0df64520a3/prepare.log`: PowerShell parse failure до запуска
  и создания resources. Не runtime/test PASS.

## Границы и следующий шаг

- Live installed LM Studio/inventory/inference/real models **NOT_RUN**: нет разрешённого
  live profile. Own WS termination не доказывает physical LMS unload/TTL/retention.
- Genuine input, advanced native profile interaction, full viewport/layout matrix,
  capture/clipboard/personal desktop **NOT_RUN**. User не назначался тестером.
- Whole S04/S07/release, S05 knowledge/OCR, packaging/installer/performance, populated
  production upgrade, OS sleep/power-loss и historical unknown fixture/PID reconciliation
  не приняты. Прежний execution4/4/61s receipt сохранён, заново не повторён.
- Причина раннего GUI исчезновения UNKNOWN; последующий bounded own close/reopen PASS
  не переписывает тот unknown. Fresh review и affected follow-up закрыты scoped;
  следующий live gate требует разрешённого собственного stand, не foundation/SDK audit.
- Codebase Memory использован для composition/authority/processing owners и связей;
  critical source сверялся с worktree. Serena Rust/JS navigation недоступен (active LS=[]).
  ast-grep `await $EXPR` по panel выявил единственный native `gui_processing` invoke и
  explicit control/replay call sites; runtime подтверждён командами, не статическим графом.
- Девять исходных планов не изменены. На writer release индекс был пуст, writer
  ничего не staged/committed/pushed. Source/known-root guard — в managed acceptance;
  parent scoped checkpoint/evidence — в [result](result.md).
