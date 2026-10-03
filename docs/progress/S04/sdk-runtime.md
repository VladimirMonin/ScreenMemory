# S04 — runnable private SDK/runtime seam

Parent: **ACCEPT_CODE_SCOPED**, [итог/evidence](result.md), не весь S04/релиз.
Ниже сохранён SDK writer handoff на входе S03 `c0648dca`,
[принятый срез](../S03/result.md), ADR-011. SDK вне WebView; его scope не добавлял
embeddings/download/jobs/search. Durable queue и GUI/CLI подключены последующими owners.

## Реализовано

- Published `@lmstudio/sdk@2.0.0`, exact standalone package/lock, 11 transitive
  official source packages; lifecycle/license preflight до dependency execution,
  frozen install/ignore-scripts. Closure729066 compressed /4180190 unpacked bytes,
  без install hooks/native executables. Checked SDK archive использован локально,
  не скачан повторно. Actual Apache2 LICENSE сохранён при metadata MIT:
  [provenance](../../../packages/lmstudio-bridge/DEPENDENCIES.json),
  [notices](../../../packages/lmstudio-bridge/THIRD-PARTY-NOTICES.md), original vendor archive/LICENSE.
- Private Node24.13.0 + SDK/module/licenses собираются offline `scripts/stage.mjs`.
  Native trusted-directory manifest pins исполняемые bytes; это не signed installer
  и не sandbox от hostile same-user. Clone реально запускается без dev PATH.
- `sm-runtime`: один owned child/Job, active-process1, commit512MiB, checked stop/wait/
  Job-empty, максимум3 explicit starts, никаких автоматических load/replay/restarts.
  ENV очищен, только OS-resolved SystemRoot; no-global-search-paths/no-addons.
- Private authenticated stdio `sm-sdk/1.1`: LE32 JSON≤4MiB + отдельный PNG≤20MiB;
  secret authority token/cookie/generation только initialization/payload, не argv/ENV/log.
  Strict schema, monotonic IDs, bounded read pump; bridge active8/mutation1/status64.
  SDK errors редактируются в closed codes, raw logger отключён.
- Явный `ws://127.0.0.1:<numeric-port>`, без discovery/remote LM Link. Explicit
  exact-local inventory→load с `deviceIdentifier:null`; specific immutable reference,
  config readback и refusal collision/mismatch/replacement. После restart нет adoption.
- PNG1024/2048 из own committed canonical через S03, StripPrivate + checksum/read lease,
  не внешний source/path/EXIF. Structured `.respond()` + image-base64; общий max1…4000,
  JSON validation и stop reason. Truncated/cancelled output не publishable.
  ACK отдельно от settlement; grace3s/transport failure fences mutation и завершает
  только собственный Node. Caller-future drop также fences lane, не backend settlement.

## Public API следующему queue owner

Исходники: `crates/runtime/src/{lib,supervisor}.rs`; bridge — `packages/lmstudio-bridge/src/`.

1. `BridgePackage::open(trusted_absolute_install_directory)` →
   `RuntimeSupervisor::new(package, selected_numeric_loopback_ws)`; `start(&dyn SecretAuthority)`.
   Authority возвращает `Zeroizing<String>`; empty token — explicit guest policy,
   не environment fallback. Off может вообще не стартовать runtime.
2. `ModelLifecycle` / `VisionInference` — native typed ports, один serial owning lane.
   `inventory(ctx,cancel)` (trait returns `RuntimeInventory`);
   `load_owned(ctx,ai_enabled,LoadPlan,&dyn LoadIntentStore,cancel)` → `OwnedInstance`;
   `observe(&instance,cancel)`, `unload_owned(&instance)`.
3. **До dispatch** `LoadIntentStore::commit_before_dispatch(endpoint,generation,&plan)`
   обязан commit через existing единственный SQLite writer, refusing unresolved earlier
   work. `record_outcome(intent_id,&Result<LoadObservation,RuntimeError>)` сохраняет
   identity/config/uncertainty. Identifier ровно `sm-<intent_uuid>`. Exact plan включает
   context/eval batch/FA/KV placement/independent K,V/positive optional native TTL; GPU backend-selected.
   `LoadObservation.native_ttl` возвращает exact requested_seconds + verification=`not_verified`,
   отдельно от reported_applied load fields. Это не host Idle/AfterBatch/KeepLoaded.
   `OwnedInstance` не Deserialize и не создаётся из inventory/старых DB rows.
4. `PreparedInferenceImage::prepare(&ImageServices,ctx,operation_id,record_id,asset_id,max_side)`
   — единственный constructor. Нужны existing ExportImages/ReadImages scopes. Borrow
   в `predict_structured(ctx,ai_enabled,&instance,&image,VisionRequest,cancel)` удерживает
   lease; **после Uncertain caller сохраняет image/operation binding до reconciliation**.
   Убийство Node не доказывает остановку LM Studio. Identity getters: `asset_id()`, `operation_id()`.
5. Load сейчас требует `LoadModelForProbe`, prediction — `RunAiProbe`+`ReadImages`,
   inventory — `ReadRuntimeInventory`. Production permission/policy/profile revisions,
   runtime-call intent/attempt/epoch, singleflight/leases и guarded publication добавляет
   queue owner; не доверять wire boolean и не держать ModelLease на всю pending queue.
   `Prediction` содержит complete/value/stop_reason/usage/reference; missing reasoning usage=null.
6. Application API/schema не renumbered/не подключены к shotd этим пакетом. Actual storage
   schema5, следующая migration **0006**, не историческая S04 schema4. Runtime private
   protocol независим от existing foundation bridge DTO. Import/export/Off не требуют SDK.

## Проверено и ограничения

Current existing-runner evidence:
`.pi/temporary/ed44d5fc-44b7-4e86-bd28-ebf3f1275286/result.json` — five affected
SDK/native steps после strong unload/explicit TTL receipt amendment, все native exits **0**:


- Pinned Node + protocol TypeScript compiler, `--project packages/lmstudio-bridge/tsconfig.json`.
- Pinned Node `--test packages/lmstudio-bridge/test/sdk.test.mjs`: **7/7** actual published
  module cases (inventory/load/readback/vision, malformed JSON/limits/cancel, replacement,
  auth/oversize/hash rejection, uncertainty fencing, offline packaged clone).
- `cargo test --offline --locked -p sm-runtime`: **1 composed** native case — OS ENV poison,
  real S03 import→source deleted→canonical PNG1024/2048/lease→packaged SDK request,
  cancellation ACK/final drain, ignored cancellation/uncertainty, restart/no adoption,
  own backend termination, canonical preservation and owned roots cleanup.
- `cargo clippy --offline --locked -p sm-runtime --all-targets -- -D warnings`, affected
  rustfmt check. Boundary check raw0 отдельно:
  `.pi/temporary/a6d612a4-5532-4b47-9cc1-27dca14a1e7c/result.json` last step.
  `native_ttl` requested900/not_verified проверен и в SDK, и в native composition.
  Production image-worker
  build raw0: `.pi/temporary/d7d23ed6-6392-4f87-bc3a-97f34e573a88/result.json` first step.

Earlier FAILs сохранены: type/path/oracle corrections и native raw101 CSPRNG failure
при полностью empty Windows ENV. Approved fix — bounded `GetWindowsDirectoryW` SystemRoot,
не inherited values; свежий native poison+clone case PASS. Raw logs остаются ignored/private.
Serena Active language servers=[]; разрешён current-source fallback. Codebase owners
Storage/ImageServices/lease/TrustedWorker и actual SDK AST callRpc mapping использованы.

**NOT_RUN:** real LM Studio/selected own model/live vision, behavior/config/TTL/reasoning
matrix, multimodal token-budget/quality measurement, signed clean-machine installer,
whole workspace/full verify/catalog regression. Fresh integrated review закрыт
позднее scoped; см. [parent result](result.md).
**Parent strong decision, не stage acceptance:** все app-initiated unload причины
(manual/host Idle/AfterBatch/reload cleanup/Off/shutdown/recovery) typed Unsupported,
без destructive SDK/REST RPC, UUID-argument fallback или operational assumption о
private alias reservation. Published unload identifier-only без atomic instance comparison;
precheck→unload не выполняется. Lease release, own Node stop и asyncDispose закрывают
локальные resources/ports, **не physical eviction**. Native TTL — отдельно `not_verified`
server policy; requested lifecycle нельзя silently ignore/switch, unsupported host mode
должен блокироваться до dispatch будущим queue/UI owner. Off/Budget100 reasoning без behavior-verified capability
не эмулируются; только явный BackendDefault. AfterBatch/KeepLoaded/TTL behavior не
объявлены реализованными host lifecycle. SDK temporary uploads не обещают zero-retention.
