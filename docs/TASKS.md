# ScreenMemory — доска работ до локального релиза

Обновлено: 2026-10-03. Цель: `muqhdldy-5nzeny`.
Исходная кодовая база: `18a2608cad800d1b3cb783d586964784e267351d` (`main`).
Scope: [ADR-011 / CN-20261002-01](adr/011-private-release-scope.md).
Исполнение: [WORKFLOW.release](../instructions/WORKFLOW.release.instructions.md).

**FTS5 — да; embeddings/семантика/sqlite-vec — нет. SDK LM Studio входит в поставку,
работает вне WebView. Визуал, сценарии и качество поиска проверяют агенты.**
Это доска реализации, не объявление готового релиза. Девять исходных планов неизменны.

## Крупные задачи

| Задача | Статус | Owner / готовность |
|---|---|---|
| Scope и доска | DONE | Parent: ADR-011, маршруты, ссылки/frontmatter/9 plan SHA и 10 локальных workflow checks прошли; это не продуктовая приёмка. |
| S01 — рабочая основа | IN_PROGRESS, не принята целиком | Sol6.1 writer → fresh Sol6.1 reviewer → parent: актуальный handoff, один host/root/DB/IPC/client, реальные gates и запуск. |
| S02 — capture/library | IN_PROGRESS, не принята целиком | Sol6.1: сквозной capture→Saved→видимый preview, режимы/hotkeys, recovery, transfer/clipboard, pin/editor, CLI. |
| S03 — импорт/изображения/экспорт | ACCEPT_CODE_SCOPED; этап IN_PROGRESS | Own files/tree→verified WebP/AVIF→actual Tauri preview→export: [приёмка/evidence](progress/S03/result.md), fresh review+P2 closure PASS. Whole S03, genuine input, production upgrade и package/performance не приняты. |
| S04 — SDK/очередь/LM Studio | ACCEPT_CODE_SCOPED; этап IN_PROGRESS | Code `0f32ad56`; [Parent приёмка/evidence](progress/S04/result.md): SDK/private runtime, durable scheduler/ModelManager/SQLite intents и реальные shared GUI/JSON CLI приняты scoped после fresh review9cc6+targeted3e43 pass/findings[]. Published-SDK synthetic execution4/4, actual shotd/CLI regression1/1, native enrollment negatives4 и исправленный schema6 smoke56/56 raw0; own rendered settings/close-reopen proofs разделены. Live LM Studio/genuine input NOT_RUN, physical unload/unsafe retention/reasoning Unsupported, TTL requested/not_verified; весь S04/поставка не приняты. |
| S05 — OCR/карточки/правки | ACCEPT_CODE_SCOPED manual-core; этап IN_PROGRESS | [Parent приёмка/evidence](progress/S05/manual-core.md): additive0007, Saved→typed карточка/категории/Keep-Inherit-Set через native GUI/JSON CLI, replay/conflicts/reopen/export при Off и effective source/debt. Fresh review + targeted P2 closure/managed settlement pass/findings[]; independent native1/1/storage7/7/Rust3/Ajv1/Python raw0. OCR/vision/preferences/quality не реализованы; genuine input NOT_PROVEN, historical native fail-fast cause UNKNOWN; этап не DONE. |
| S06 — FTS5 и качество поиска | PENDING | Sol6.1 + оценка parent: lexical/exact/filters, обновление после правок/удалений, размеченный RU/EN corpus, ranking и latency. |
| S07 — GUI/CLI/MCP | PENDING | Sol6.1: полный агентский сценарий, удобный GUI, общий host, scopes, JSON/exit parity и image resources. |
| S08 — сохранность/релиз | PENDING | Sol6.1 → reviewer → parent: финальная регрессия, isolated backup/restore/purge/upgrade, Windows installer/staging, SHA и отчёт. |

Задача закрывается только вместе с требуемыми проверками и evidence. PASS отдельного
среза не закрывает всю Sxx. Embedding/vector требования исходного S06 исключены
по ADR-011; FTS5/SDK/packaging proofs остаются у реализующих их S06/S04/S08.

## Проверка этого организационного checkpoint

- `.pi/validate-release-scope.py`: ссылки пяти документов, frontmatter/каталог,
  SHA-256 девяти исходных планов и native Sol6.1 profiles — exit 0;
  `.pi/scope-check-e8cb7cbb7c4a4bf18941647d065e46ce/result.json`.
- Pinned Node `.pi/orchestration/test-milestone-workflow.cjs`: 10 checks exit 0,
  включая явный Sol6.1 route, test-evidence schema и отказ старого provider/fallback.
- Pi `action:validate` milestone workflow: ok=true, только предупреждение о dynamic
  spawn count; runtime остаётся владельцем launch preflight. Дети ещё не запускались.
- `git diff --check`: exit 0. `.pi` profiles/helpers — локальные ignored artifacts,
  не production-код и не часть релиза; устойчивый маршрут закреплён в инструкции.

## Что уже существует

- Workspace Rust + Tauri/React/TS; pinned Windows build helpers и контрактные схемы.
- Root isolation/HostLease, один SQLite worker, миграции, credentials, bounded framing,
  sealed Windows named-pipe/session и shared client.
- Реальные `shotd`/`shot`, диагностическое окно, native capture, exact-lossless WebP,
  записи/assets/operation receipts и существенная storage recovery-реализация.
- [GUI evidence](progress/S02/gui-gallery-preview.md): первые 24 записи и bounded
  ≤8 MiB WebP preview; owner listing исправлен; **целый synthetic WebP виден в настоящем
  Tauri-окне**, не только mock browser. Последний срез — `18a2608c`.
- Последний GUI frontend: typecheck/lint/build exit 0, 15/15 tests; offline GUI build,
  owned self-check/window/reopen/host stop exit 0. Contracts implemented subset PASS,
  stage IN_PROGRESS. Это исторические результаты указанного snapshot, не свежий
  workspace gate и не доказательство всего capture→preview.
- [F06/F07 process crash](progress/S02/t09n4-process-crash.md): own test child termination
  и повторный recovery, узкое storage-only доказательство; commit `06763156`.

[S01 result.md](progress/S01/result.md)/[result.json](progress/S01/result.json)
обновлены по фактическому Git/runner/runtime. Foundation scoped code commit **`99df8f3`**: composed smoke 6/6,
65 cleanup-safety checks и 12 two-root/mutation cases raw 0; [P1/P2 correction](progress/S01/foundation-cleanup-fix.md)
закрывает misbound cleanup и финализацию второго fixture. Sol6.1 scoped accept,
parent post-P2 verify36/runner regression raw0: **ACCEPT_CODE_SCOPED**.
Вся S01 не объявлена принятой; historical unknown synthetic residues не скрыты.

## Текущий продуктовый milestone

Foundation boot/verify/helper seam принят scoped (`99df8f3`, handoff `de41adb`),
не вся S01. Native workflow `e7c30bf9-0e73-4905-95df-58c04b660294` завершился
на Astra decision `blocked`: без разрешённого собственного guest writer не запускался.
[Live-stand blocker](progress/S02/isolated-live-stand.md) остаётся у capture/input;
Windows Sandbox preflight raw0 выявил disabled feature, не runtime PASS.
**Сейчас:** S03 import/preview/export и S04 SDK/processing/shared GUI/JSON CLI
приняты как полезные code slices, не как полные stages. [S04 parent result](progress/S04/result.md)
связывает [SDK](progress/S04/sdk-runtime.md), [execution](progress/S04/processing.md),
[interfaces](progress/S04/interfaces.md), fresh review и affected schema-smoke correction.
Code checkpoint **`0f32ad56`**, все writers/reviewers завершены, source released.
S05 manual-core принят scoped: карточка, категории и manual overrides через общий
GUI/CLI при AI Off, versioned effective source и revision guards;
[evidence/границы](progress/S05/manual-core.md). Два P2 (Unicode parity и crash-report
classification) закрыты; report-delivery FAIL сохранён, same-native settlement PASS.
Все writers/reviewers завершены, parent фиксирует принятый логический срез.
OCR/vision integration и supported identity остаются отдельным следующим owner;
полный S05 не закрывается manual-only срезом. Затем production FTS5.
Capture/input/live-model/unload/TTL blockers остаются локальными, не паузой всей цели.
Historical unknown fixture/PID cleanup BLOCKED/NOT_RUN; не повторять принятый SDK
bootstrap и не выдавать bounded checks за whole S04.

| Lane | Решение и scope | Cwd / isolation | Gate / handoff |
|---|---|---|---|
| Foundation writer/reviewer → parent | DONE scoped: boot/verify/handoff и P1/P2 cleanup | Source released; `99df8f3`, report `de41adb` | Verify36, composed6, review accept; полная S01 IN_PROGRESS |
| S02 isolation oracle | DONE consultation, live BLOCKED: personal WinSta0 race не разрешён | Read-only, source released, writers не запускались | Astra decision + OS metadata preflight; actual guest availability ещё не доказана |
| S02 support/integration/review | DEFERRED до собственного чистого guest/session, capture требования сохранены | Без source writers и без live pixel-read/input | Same-frame four modes/IDs/hash/real input/restart остаются NOT_RUN |
| S03 image owner | READY handoff: decoding/metadata/normalization, WebP+AVIF, contained worker | Source сохранён, sequential owner | Synthetic codec/worker checks; package/stage acceptance отдельно |
| S03 ingest owner | READY handoff: retained source→verified canonical→no-replace→commit/Saved, durable control/export | Source сохранён, один прежний SQLite/file owner | Duplicate/failure/cancel/protected-target/TTL-lease tests; production populated upgrade guarded |
| S03 interface integration | ACCEPT_CODE_SCOPED: common RPC/client/JSON CLI + typed/native-path GUI, source-aware pagination, bounded AVIF derivative | Source released; [факты/остатки](progress/S03/interfaces.md) | Actual own Tauri DPI144 RGB preview/source deletion/export/reopen/cleanup1168; final Rust/frontend raw0, comments-only schema drift исправлен, contracts raw0 |
| S03 reviewer → parent | DONE scoped: fresh review и retained P2 follow-up PASS/findings0 | Все writers завершены; parent принимает/коммитит | Оба report defects исправлены, public-progress invariant/empty CLI raw7 независимо проверены; whole stage не объявлен DONE |
| S04 writers/reviewers → parent | ACCEPT_CODE_SCOPED: real installation authority, SDK/durable execution, common typed GUI/JSON CLI | Native SDK/execution/interfaces CHECKED, fresh9cc6/targeted3e43 pass; source released | [Final evidence/остатки](progress/S04/result.md); independent actual native case1/1+4 enrollment refusals; schema-smoke RED1 сохранён/GREEN56 raw0. Whole S04/LM live/unload/TTL/genuine input не приняты |

Завершённый foundation был single-seam. S03 image, ingest и interface integration
имеют последовательных owners и durable handoff, без пересекающихся source writes.
Нет назначения одному writer «сделать S01–S08». Новые storage/runtime/
search контракты получают свои крупные задания и integration handoff, не writer swarm.
Pi: `openai-codex/gpt-6.1-sol` для реализации/review;
`openai-codex/gpt-6-astra` для oracle при реальной неопределённости.

## Открытые риски и конкретные действия

| Остаток | Состояние / следующий шаг |
|---|---|
| Единый live capture→preview и click-to-preview | BLOCKED/NOT_RUN: [изолированный stand](progress/S02/isolated-live-stand.md), own guest/session ещё не подтверждён; Windows Sandbox disabled. Не включать OS features и не просить владельца кликать вместо агента. |
| Native WM_HOTKEY, menus/first drag/Esc, четыре режима, mixed DPI/negative coordinates | Код/частичные proofs не вся Windows matrix; подготовить собственные synthetic окна и проверить. |
| Recovery startup/status/retry | NOT_WIRED полностью; интеграция после runnable цепочки, reuse уже принятых capability/SQL частей. |
| Условная pending→Saved, hostile prior writable mappings | [Условная модель](progress/S02/t09n3b2-conditional-saved.md). Нельзя объявить unconditional proof или переписать guard ради PASS. |
| Power loss/NTFS и глобальный RSS ≤1 GiB | NOT_RUN/NOT_PROVEN; process-kill не равен отключению питания. Отдельный риск, не остановка независимого development. |
| Временный recovery codec cap 1 MP | Не production 96 MP acceptance; решить вместе с реальными memory/recovery bounds. |
| Legacy host asset read | Full-file-per-chunk allocation устранена fixed scratch/range buffer; повторное checksum scanning остаётся. S03 preview — отдельный bounded cached derivative; full transfer/performance позже. |
| Pin/editor/clipboard/transfer, полный CLI/MCP | Не приняты; реализовать соответствующие milestones, не назвать ранний bounded preview полным T10/T11/S07. |
| Global fmt drift и прежний default-thread codec overflow | Исторические baseline проблемы, не текущий foundation blocker: verify36 и runner regression raw0. Desktop-reading target default-off; live matrix остаётся NOT_RUN. |
| Общий preview target занят чужим host | Не останавливать чужой процесс. Использовать отдельный owned target/root; exact UUID, финализация только собственных handles. |
| LM Studio/models и clean Windows test environment | Проверить доступность read-only, не скачивать тайно. SDK/runtime bundling и независимую работу продолжать; реальный proof без доступа не выдумывать. |
| Исторические synthetic credentials | Malformed reviewer canary и pre-P2 absent fixture без сохранённого ID — UNKNOWN/NOT_RECONCILED; новые exact1168 proofs не доказывают старую absence. Enumeration/sweep без нового решения запрещены. |
| Clipboard isolation | Desktops WinSta0 делят clipboard: никаких Copy/EmptyClipboard в этой capture-wave. Нужна собственная station/session/VM и host/consumer той же station. |

## Обязательная финальная оценка

- Агенты сами проверяют GUI по собственным physical screenshots/сценариям, preview/DPI
  и ошибкам; mock отделён от live. Никогда не отправлять ввод в чужое foreground окно.
- FTS5: заранее размеченный corpus, ожидаемые IDs/порядок, RU/EN/фразы/коды/фильтры/
  no-results, freshness и latency. Сохранение/поиск без model calls проверяются отдельно.
- Backup/restore/upgrade — на isolated synthetic данных; installer/staging без dev tools.
- Итоговый report: actual SHA/версии, команды/raw exits, evidence, остаточные риски.
- Объективный blocker локализуется; независимая работа продолжается, при тупике — Astra
  oracle. Обязательный непроверенный case не повышается до PASS, задача — не до DONE.
