# S07 authority/core — partial checkpoint, SOURCE_RELEASED

База `151644b1de30244972eb4a90ca07d67a654a22dd`. Parent приостановил goal во время
согласования compatibility corrections. **PARTIAL, не READY/не приёмка S07**;
CLI/reviewer не разрешаются этим checkpoint. После pause продуктовые изменения,
новые тесты/генерации/сборки не выполнялись. Parent ADR-012/TASKS сохранены.

## Сохранённый candidate

- Additive0009 под прежним StorageWorker/HostLease: profile64/connection128,
  immutable principal/history/receipts/events, metadata-only policy, durable
  enrollment digest/IDs и фиксированные operation-scoped cleanup targets.
- Typed exact agent credential; native environment/ID guards и confirmed-absence
  agent-only delete. Owner targets byte-compatible; нет enumeration/fallback.
- Host enrollment commits plan до vault write; matching readback до activation.
  Accidental missing-slot retry не генерирует replacement; revoke/disable сначала
  COMMIT/publish, cleanup потом. Current durable authority вместо verifier snapshot.
- Application1.0/16 capabilities, Owner ManageAccess, shared client access port;
  agent principal из durable profile. Session guarded delayed delivery держит read
  permit через framing; writer owns exclusive permit до COMMIT даже при drop caller.
- Agent connection/security revision/generation связаны с search/cursor/cancel.
  Source ANY-required-scope исключает целую строку до top-k; global category
  maintenance catalog для agent явно запрещён. Owner ListRecords не расширен.
- CLI/parser/MCP/UI/SDK/model/OS authorities не изменены. Никаких новых dependencies.

## Реальные результаты (raw exits)

Immutable evidence root:
`.pi/temporary/4f0233ce-b4d3-4adc-8740-cf29448a2bd6/`.
Каждый UUID ниже содержит actual argv, raw exit и SHA-256 stdout/stderr в `result.json`.

| Проверка | Artifact UUID | Результат |
|---|---|---|
| `cargo check --offline --locked` шесть affected owners | `c778104f-43c3-4b32-b457-6b16592e621b` | raw0, default production compilation |
| `cargo test --offline --locked -p sm-host -p sm-storage -p sm-credentials -p sm-protocol s07_ -- --test-threads=1 --nocapture` | `777b54e2-981f-4aec-8c50-94fa53022588` | raw0, 10/10 новых tests |
| Финальный host `--lib s07_`, реальные ImageServices preview + native scoped session | `dd1032d4-64fd-4b2d-9053-284338a996d2` | raw0, 2/2; owner write/preview и connectionB продолжают после revoke |
| Pinned exact-file rustfmt edition2021/skip_children | `01a33e85-6df3-4845-9ea4-d6333a6d219e` | raw0 |
| Affected owners `cargo clippy --offline --locked --all-targets -- -D warnings` | `df1e6bfc-808e-4f53-b056-a473b93d5dfb` | raw0 |
| Fresh Rust/TS generation, inspect/publish только двух application outputs | `9d28100b-b75e-4c9b-95bc-d631cf94fea8`, `506bf8e3-bfb8-43c3-8ab5-d2e50678fdd0` | raw0/0; bridge/errors bytes unchanged |
| Pinned Node protocol Ajv tests | `d0723038-da43-452f-b093-53d20c6e11ff` | raw0; shared access negatives/bounds |
| Explicit Python3.14 shared schema fixtures | `a5d36ff4-ac9c-4159-b48d-640374fd220d` | raw0 |
| Protocol typecheck | `fa5ace42-d4bf-44e2-a8da-976252ce968c` | **raw2 FAIL**: existing index exports CardEdit/CategoryMutation отсутствуют в generated union exports |
| Python boundary guard | `09b34077-820a-4d2e-8b90-d897b678fd5f` | **raw1 FAIL**: stale protocol manifest allowlist, до изменённых access boundaries |
| Application/protocol/session affected tests | `f67b1076-0889-4715-a4db-21c6072b152b` | **raw101 FAIL**: 3 raw-wire cases с прежним hardcoded CAPS; остальные executed groups PASS |

Три узких corrections **не разрешены в pause и не выполнены**: TS public aliases,
current pinned dependency guard, wire CAPS16. Полный final build/schema-check/lint и
повтор failing gates NOT_RUN. Whole workspace/foundation/SDK/perf/gold/GUI не повторялись.
Compile/argv/setup failures не объявлены TDD-red; прежние raw101/2/1 сохранены, не
заменены PASS. Неверно преобразованный shell `/d /c` initial raw0 не build evidence.

Новые tests: protocol2 (closed requests/native revision); storage3 (caps/history/replay,
planned digest/collision/denied secret, transactional rollback), migration1 (8→9 rollback,
UUID/checksum prefix), source-scope1 (before top-k/card/catalog/stale cursor/re-enable),
host2 (three enrollment faults/restart/sameIDs/cleanup failure; delayed revoke+disable,
publication ordering, owner/connectionB), native vault1 (exact key/environment/collision/
absence). Store faults не названы Win32 proof. Genuine input/live model не проверялись.

## Сохранность / освобождение

`0a43d11f-aef9-4f52-bb67-cb9e201ffd86/checkpoint.json` и `source-after.json`:
HEAD прежний, index unstaged/unchanged, девять планов и SQL0001–0008 unchanged,
Parent docs unchanged, нет удалённых source files. Exact шесть logged own roots,
включая failed native assertion fixture, проверены absent. Native Test agent slot
имеет отдельное `NATIVE_AGENT_SLOT_ABSENT` после actual CredRead absence. Guard
вооружался сразу после write; чужие/native historical slots не перечислялись.

Все gate subprocesses реально возвратились; successful own session task awaited,
StorageWorker shutdown закрывает readers/SQLite/lease до root cleanup. Legacy session
fixtures закрылись штатным Drop; их exact root IDs отдельно в logs не записаны, поэтому
это не blanket exact-path absence proof. Процессы/handles/эндпоинты после pause не
создавались. **SOURCE_RELEASED**: writer больше не изменяет продукт; checkpoint сохранён
для same-native продолжения только после явного resume. Fresh review не выполнен;
весь S07, CLI и релиз не приняты.
