# S04 — приёмка рабочего SDK / processing / интерфейсного среза

Дата: 2026-10-03. Решение parent: **ACCEPT_CODE_SCOPED** после fresh review и
исправления schema-smoke oracle. **Весь S04 и релиз остаются IN_PROGRESS.**
Baseline: `c0648dca3abbeb75e9632d5ebea47c4eb9341555`.
Code commit: **`0f32ad56f40eb6020a7f82c2f34ae0e7c0d1e369`**,104 files;
`feat(processing): встроить SDK, durable очередь и общее управление`.
Post-commit tracked tree чист; push/публикация не выполнялись.

## Что принято

- Published SDK2.0.0, pinned private Node, offline staging/лицензии вне WebView:
  [SDK handoff](sdk-runtime.md). Actual Apache2 LICENSE сохранён, metadata MIT не
  выдаётся за фактическую лицензию. Staged clone запускается без dev PATH;
  installer/clean-machine поставка ещё не приняты.
- Единственный SQLite owner, additive `0006_processing`, bounded scheduler32,
  leases/attempts/epochs, ModelManager, intents до dispatch, guarded publication,
  cancellation/uncertainty/reopen: [execution handoff](processing.md).
  `0001`–`0005` и девять исходных планов не изменены.
- Реальный installation bootstrap `shotd`, bounded stdin-only backend credential,
  trusted fixed configuration, общий typed client и GUI/JSON CLI:
  [interfaces](interfaces.md). Off/overview/profile save не грузят модель;
  explicit inventory может стартовать bridge, но не load/adopt.
- Actual public-SDK technical self-test на собственном WS fixture и Saved canonical;
  idempotent operation/replay, bounded queue32+3 и control/revision-conflict.
  Это не LM Studio/model vision и не S05 description success.
- Parent видел physical-DPI own-HWND settings screenshot. Readable rendered settings
  и guarded close/reopen/cleanup доказаны разными receipts; пустая поздняя поверхность
  не выдана за rendered UI, advanced controls/genuine input не доказаны.

## Проверки, действительно исполненные

Raw logs приватны под `.pi/temporary/`; полные команды — в owning handoffs.
Проверки SDK/foundation/full workspace не повторялись без конкретного дефекта.

| Проверка | Outcome / evidence |
|---|---|
| SDK TypeScript/actual module/native composition/clippy | raw0; SDK7/7, native1/1; `ed44d5fc-44b7-4e86-bd28-ebf3f1275286/result.json` |
| Execution parity/strict shared fixtures/default+seams clippy | raw0; финальные refs в [processing](processing.md) |
| Final serial execution suite и восстановленная production build | raw0;4/4,115.05s, actual61s reactor gap; default8-package build0; `14e91fc6-3f19-4d61-aff6-a813eb2da12e/result.json`. Не OS sleep |
| Native GUI/RPC, frontend, CLI, protocol | raw0;12/12,19/19,8/8,8/8; [interfaces](interfaces.md) |
| Writer actual default-shotd / JSON CLI / staged SDK | raw0;1/1,27.99s; `f239e72e-70d4-40b8-af1b-ce004eed7bcd/default-actual-host-cli.log` |
| Fresh reviewer независимые default build и actual interface regression | raw0;1/1,28.52s; `01c6cfd2-662e-4fa0-92c9-1d91bf0e70c9/{default-build,actual-interface}.log` |
| Fresh reviewer четыре actual enrollment refusals | empty/newline/oversized/invalid UTF-8 stdin: каждый exit6, stdout empty, fresh root absent, без synthetic ENV-token fallback; `01c6cfd2-662e-4fa0-92c9-1d91bf0e70c9/enrollment-negatives.json` |
| Parent stale schema-smoke RED | raw1/SCHEMA-VERSION: actual init уже schema6, oracle требовала5; `d3a3fea9-9870-4016-8b50-6a0bea8c2334/smoke/result.json`. FAIL сохранён |
| Parent affected smoke GREEN | raw0;56/56 steps; точное expected6, прежние doctor/duplicate/autostart/no-start/stop/restart/nochild/cleanup guards; `7a210941-2b0b-4993-a059-b9daf52aa25a/smoke/result.json` |
| Exact RED fixture finalization | raw0, exact synthetic credential absence1168/root absent; `d3a3fea9-9870-4016-8b50-6a0bea8c2334/red-own-cleanup.json` |
| Fresh review / targeted follow-up | native `9cc6c133…` и retained `3e43c383…`: **pass/findings[]**, terminal observed. Follow-up не запускал тесты заново |

Owning review artifact:
`61c4854f-a3ed-42c6-a2bf-dfceaaf17b1e/functional-review/review.json` в managed
subagent outputs. Original fresh report сохранён до follow-up в
`.pi/recovery/s04-interfaces-timeout-6e76a6d7/original-functional-review-9cc6.json`.

## История не переписана

SDK report/parser failures, два truthful partial execution checkpoints и interface
7200000ms timeout остаются FAIL. Retained native continuation закрывала только
отсутствовавшие gates/report, не reset/reimplementation. Final interface settlement
`8d2a79a7…` CHECKED сравнила102 captured files byte-for-byte; source не менялся.
Parent после fresh release исправил exact schema5→6 smoke oracle/comment,
ложный no-SDK-linked note и соответствующий HOST instruction text. Targeted follow-up
подтвердил отсутствие ослабления guards. Staged diff затем выявил CRLF в двух новых
SDK documents (raw2): нормализованы только line endings DEPENDENCIES/SDK-LICENSE,
JSON values/полный license text сохранены; исходные archive/license bytes внутри tgz
не изменены. Byte equality после удаления CR и JSON value equality проверены;
archive byte-unchanged. Notice явно описывает нормализацию; runtime-код не менялся.
Final staged diff check raw0, exact104-file staging, no unstaged source.
RED2/GREEN0 logs сохранены в `.pi/recovery/s04-interfaces-timeout-6e76a6d7/`. Новые root/credential proofs не закрывают
исторические unknown/non-UUID fixtures или отсутствующие записи PID.

## Что НЕ принято и что дальше

- **Physical unload — Unsupported для всех причин.** SDK2.0.0 identifier-only unload
  не имеет atomic expected-instance/CAS; observation/UUID/lease release/Node stop не
  дают ownership/eviction proof. Host Idle/AfterBatch/KeepLoaded не реализованы.
- Native TTL — exact requested seconds / `not_verified`, отдельная server policy,
  не settlement. Unsafe retention/reasoning Off/Budget100 отказывают до dispatch;
  только explicit NoHostEviction/BackendDefault, без silent fallback.
- Authorized live LM Studio/model vision/config/quality/physical TTL **NOT_RUN**.
  Нет разрешённого endpoint/model; чужие instances не перечислялись/не загружались.
- Advanced native GUI/genuine input/full viewport **NOT_PROVEN**; earlier GUI
  disappearance/exit **UNKNOWN**, поздний lifecycle PASS не объясняет его.
- Full verification/catalog regression, actual OS sleep, performance/RSS,
  production upgrade/backup, installer/clean-machine release **NOT_RUN**.
  Conditional Saved/recovery и capture/live-stand ограничения прежних stages сохранены.
- S05 Analyze пока `blocked:handler_unavailable`. Следующий независимый milestone —
  OCR/knowledge/manual overrides и revision-safe publication; затем production FTS5.
  Полный goal `s04-lmstudio` не закрыт этим code checkpoint.
