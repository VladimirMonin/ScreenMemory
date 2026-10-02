# S01 — воспроизводимая рабочая основа, handoff 2026-10-02

**S01 IN_PROGRESS, не принят целиком.** Этот handoff относится к base
`9a7944d5cf33563ccb598e5b7e9dc035d8190e4e`, кодовому baseline
`18a2608cad800d1b3cb783d586964784e267351d`; foundation code commit
**`99df8f3c0e8d6c3a57bc6afde567829c1851a366`**, ACCEPT_CODE_SCOPED.
Writer ничего не staging/commit. Fresh Sol6.1 review + same-role P2 challenge завершены:
parent **ACCEPT_CODE_SCOPED**, не full S01. Commit выполняет parent.
[Машинный отчёт](result.json), [доска](../../TASKS.md), [ADR-011](../../adr/011-private-release-scope.md).

## Что действительно существует

Root isolation/HostLease → один SQLite worker (storage schema **4**, config schema **1**)
→ enrollment/secure named pipe/framing/session → общий `ApplicationClient` → реальные
`shotd`/`shot` и Tauri/React GUI. GUI закрывает только своё соединение; повторное окно
и CLI подключаются к тому же host. AI выключен, bridge не стартует.

T05c/T05d/T06a–T06d уже не незакоммиченные кандидаты: credentials `860aba26`, framing
`ca1f154d`, pipe `bfb2f486`, host/CLI/shared session `e5cecff1`, GUI `fadbca7a`;
поздние capture/WebP/library/gallery срезы также существуют в baseline `18a2608c`.
Это committed code slices, **не** весь принятый S01 и не новое утверждение о полноте
их прежнего review. T01/T02b/T03b/T04a/T04b/T05a checkpoints сохраняются в JSON.
Старые component reports не переписаны: их raw evidence относится только к их snapshot.
Прежний aggregate handoff доступен в Git по `9a7944d:docs/progress/S01/result.*`;
его candidate/no-host/no-GUI утверждения superseded этим отчётом.

## Изменения seam

- Verify/preview используют pinned Rust 1.98.1, Node 24.13.0/pnpm 10.32.1 и отдельный
  `.pi/staging-target-t09b`; Cargo offline также внутри вложенных pnpm commands.
  MSVC bootstrap failure возвращается немедленно. Чужой старый preview host не тронут.
- Offline больше не запускает desktop-reading `live_capture`: target требует explicit
  non-default `isolated-desktop-tests`; CAPTURE-NATIVE — `--lib`. Metadata, default
  workspace list-only inventory и compile-only refusal raw **101** проверяет
  `scripts/test-offline-safety.ps1`. LIVE остаётся **NOT_RUN**. Feature разрешён
  только будущему owned isolated Windows S02/S08 harness, не личному desktop.
- Исправлены реальные устаревшие smoke schema assertion (1→4), single-owner cardinality,
  error cleanup после непроверенного stop, pinned Node для frontend subprocesses.
- GUI wrapper не просит владельца кликать: bounded own-handle close. После P1 review
  cleanup arm разрешён лишь полным marker/tree/init-binding/config validator до connect;
  rejected/misbound config ничего не удаляет. Tuple/hashes frozen, mutation сохраняет
  local host/key/root с BLOCKED. Неизменные inputs сохраняют stop→absence→remove.
  [P1 correction](foundation-cleanup-fix.md): 12 two-root/mutation cases и старые failure paths.
- Physical PNG — только PrintWindow проверенного собственного HWND, DPI-aware,
  **не desktop capture**. Production capture/storage/IPC поведение не менялось.
- Восемь фактически failing Rust files исправлены только rustfmt. Каждый текущий
  файл побайтово равен pinned rustfmt(original copy): proof ниже, semantic edits нет.

## Проверки и immutable private evidence

Все пути ниже repo-relative, ignored `.pi`; raw logs/PNG не публикуются автоматически.
Полные step argv/raw exits/hashes находятся в JSON, а не в этом кратком тексте.

| Проверка | Реальный outcome / evidence |
|---|---|
| Initial fmt / correction | raw 1 → apply 0; `.pi/temporary/8a223126-7b6e-48cf-b570-d2636628258a/fmt.log`; mechanical-only proof 8/8: `.pi/temporary/a9765035-e54b-4907-8420-8948084d6010/result.json` |
| Default-thread codec | raw 0, без stack override/serialization/skip; `.pi/temporary/b2a00819-7669-44d0-abe2-7e3cacb235a8/codec-default.log` |
| Safe foundation verify | raw 0, 36 gates including fmt/Clippy/workspace, native credentials/framing/pipe, frontend build/typecheck/lint/test, contracts/schema/boundaries, plan/source PRE/POST; `.pi/temporary/cd1b3e8d-b094-4073-a1d4-66aba4d2b1e1/result.json` |
| Runner regression | raw 0; `.pi/test-verify-59e81948ec254c7882e86762ce25343b/result.json`; real exit7/fail-fast, timeout/spawn/redaction, copied-plan/source mutation RED, preview safety, exact argv and default live denial |
| Final composed smoke | raw 0, five cases: host/CLI 0, GUI selfcheck 0, window/reopen 0, connect-failure cleanup expected 1, window-spawn-failure cleanup expected 1; `.pi/temporary/d55f6083-b6b4-4900-aa64-2c3c70d73599/result.json` |
| Physical GUI | DPI144, **1462×1136**, ready, empty synthetic library; `.../d55f6083-b6b4-4900-aa64-2c3c70d73599/GUI-WINDOW/window/{result.json,owned-window.png}`; no capture button/input/clipboard |
| Final current-source gate | Authoritative raw outcome/steps: `.pi/temporary/182a9fa1-f426-42d9-a314-a43f8db9f770/result.json`; resolves final inventory guard and this handoff's bytes; source writes never concurrent with suite |

Composed smoke доказал init/connect/status/doctor, duplicate refusal, no-start,
shutdown receipt → process/endpoint gone, тот же durable library ID на restart,
закрытие GUI → тот же host на reopen. Все его exact Test credentials/roots и
собственные GUI процессы финализированы. В логах — raw cleanup exits/readback 1168.

## Parent correction checkpoint

Post-P2 composed regression **6/6 raw 0**:
`.pi/temporary/95051d2a-990e-410e-8dfa-a6435f974123/result.json`; includes 12-case real
A/B isolation suite. Existing safety **65 checks raw 0**:
`.pi/temporary/41469801-1bf2-4200-9a68-4d76e414961e/result.json`. Physical owned GUI
1462×1136/DPI144, ready/empty synthetic library; parent inspected PNG, SHA256
`6243fa4209535a391581a508af1cd95eb550c58e59f0ac257674241b9c5689be`.
Historical five-case smoke remains its original snapshot, not this correction proof.
Sol6.1 подтвердил P1, нашёл и повторно принял P2 fix второго fixture key cleanup;
оба новых exact smoke keys независимо absent1168. Parent verify **36 gates raw0**:
`.pi/temporary/cd649d62-c5d9-4794-b518-0cacd75eb9cf/result.json`; runner regression raw0:
`.pi/test-verify-33b79ace36b5467bbf960b51a58bc22d/result.json`.
`b18036ea` не full key-disposal proof: потерянный absent ID, возможный residue UNKNOWN;
malformed reviewer canary тоже UNKNOWN/NOT_RECONCILED, никаких широких deletes.
Parent ACCEPT_CODE_SCOPED; no full S01 upgrade.

## Safety incident / invalid baseline evidence

До parent steering был запущен старый unconditional `cargo test --workspace --locked
--offline` (raw 0): `.pi/temporary/1e01b8dd-946a-4c1f-9a8a-4b87820277d3/workspace-default.log`.
Он dispatch desktop-reading native integration tests на текущей Windows session.
Это **не безопасный offline PASS**, не capture acceptance; parent уведомлён сразу.
После steering target gated, дальнейших desktop-freezing запусков нет. Никакие
pixels этого запуска не сохранены как продуктовый/визуальный proof.
Первый последующий safe verify прерван внешним 120s tool deadline:
`.pi/temporary/ab1bbbb4-1c53-4010-8b73-ffffb9d337d1/` без финального result/raw wrapper
exit — **NOT_COMPLETED**, не PASS; повторный новый UUID gate завершён raw 0.
Первый smoke raw 1 обнаружил stale schema assertion; `.pi/temporary/8cedd012-c988-4347-92d4-b07b74e24954/result.json`.
Его единственный initialized synthetic root убран ordered owned cleanup; исправление
доказано fresh smoke, прежнее evidence не перезаписано.

## Что не выполнено / следующий owner

Embeddings/семантика/sqlite-vec **EXCLUDED** по ADR-011, не blocker. SDK/runtime
bundling/vision — S04, FTS5/index/RU/EN corpus — S06, installer/clean Windows/backup/
restore/upgrade — S08: **NOT_RUN**, не PASS. AVIF/import — S03. Whole S01 G01–G10 не
повышены до ACCEPTED. Полный capture→Saved→видимый preview, isolated desktop native
matrix, recovery startup wiring, memory/power-loss и existing host asset-read bounds
здесь не принимаются. Исторический GUI WebP proof — [S02 gallery](../S02/gui-gallery-preview.md).

Code intelligence: Serena действительно вернул `Active language servers: []`;
разрешённый fallback — Codebase graph/current source. Graph нашёл preview owners,
`GuiSession`/shared client и freeze owner; ast-grep подтвердил `connect_session` в GUI
и production `FrozenFrame::freeze` в capture host. Статика не выдана за runtime proof.

Первый review вернул fix (P1), следующий закрыл P1 и нашёл P2, final challenge
принял оба исправления. Parent принимает только этот seam, сохраняет raw failures и
исторические unknown residues. Code commit `99df8f3` создан локально, push не было.
Следующий шаг: owned-isolated capture milestone,
а не автоматическое объявление всей S01/S02 готовой.
