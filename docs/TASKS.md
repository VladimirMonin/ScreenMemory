# ScreenMemory — доска работ до локального релиза

Обновлено: 2026-10-02. Цель: `muqhdldy-5nzeny`.
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
| S03 — импорт/изображения/экспорт | PENDING | Sol6.1: файлы/папки с прогрессом/отказами, собственные WebP/AVIF, metadata/orientation, resize и экспорт. |
| S04 — SDK/очередь/LM Studio | PENDING | Sol6.1: bundled SDK/runtime, реальный vision API, профили, Automatic/Manual/Off, pause/cancel/retry, load/unload/TTL. |
| S05 — OCR/карточки/правки | PENDING | Sol6.1: рабочий OCR/vision, категории, schema validation, manual overrides и revision guards. |
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
не вся S01. **Сейчас:** агентский isolated synthetic capture→Saved→library→preview.
Native workflow `e7c30bf9-0e73-4905-95df-58c04b660294` запущен; oracle пока
согласует safety contract, runtime capture ещё NOT_RUN. Не возобновлять цепочку
микросрезов recovery до полезного сквозного результата.

| Lane | Решение и scope | Cwd / isolation | Gate / handoff |
|---|---|---|---|
| Foundation writer/reviewer → parent | DONE scoped: boot/verify/handoff и P1/P2 cleanup | Source released; `99df8f3`, report `de41adb` | Verify36, composed6, review accept; полная S01 IN_PROGRESS |
| S02 isolation oracle | RUNNING: Astra выбирает конкретный guarded launcher/desktop; parent подтверждает до исполнения | `C:/PY/ScreenMemory`, read-only, никакого pixel-read/input | Actual station/thread/input-desktop, full synthetic background, bounded restore/own watchdog contract |
| S02 desktop support | PENDING после подтверждённого oracle решения: один launcher/fixture lifecycle/privacy seam | Один Sol6.1 writer; opt-in test guards, fresh children и own handles | Runnable handoff, safety negatives и raw outcomes; compile не runtime PASS |
| S02 integration | PENDING после ready support: четыре режима→Saved→list/get→настоящий Tauri preview | Последовательно, один Sol6.1 writer после освобождения source | Same-frame pixels/IDs/hash, genuine guarded input, empty→new/first24 bounds, restart/no duplicates |
| S02 reviewer → parent | PENDING после integration: fresh Sol6.1 read-only | Source writes запрещены во время финальных integrity gates | Scoped verdict; parent оценивает physical screenshot/pixels и принимает, не whole-S02 по одному case |

Завершённый foundation был single-seam. В новой wave test-support и product integration
имеют последовательных owners и durable handoff, без пересекающихся source writes.
Нет назначения одному writer «сделать S01–S08». Новые storage/runtime/
search контракты получают свои крупные задания и integration handoff, не writer swarm.
Pi: `openai-codex/gpt-6.1-sol` для реализации/review;
`openai-codex/gpt-6-astra` для oracle при реальной неопределённости.

## Открытые риски и конкретные действия

| Остаток | Состояние / следующий шаг |
|---|---|
| Единый live capture→preview и click-to-preview | NOT_RUN целиком. Агент выполняет на собственном synthetic desktop/окне, не просит владельца кликать. |
| Native WM_HOTKEY, menus/first drag/Esc, четыре режима, mixed DPI/negative coordinates | Код/частичные proofs не вся Windows matrix; подготовить собственные synthetic окна и проверить. |
| Recovery startup/status/retry | NOT_WIRED полностью; интеграция после runnable цепочки, reuse уже принятых capability/SQL частей. |
| Условная pending→Saved, hostile prior writable mappings | [Условная модель](progress/S02/t09n3b2-conditional-saved.md). Нельзя объявить unconditional proof или переписать guard ради PASS. |
| Power loss/NTFS и глобальный RSS ≤1 GiB | NOT_RUN/NOT_PROVEN; process-kill не равен отключению питания. Отдельный риск, не остановка независимого development. |
| Временный recovery codec cap 1 MP | Не production 96 MP acceptance; решить вместе с реальными memory/recovery bounds. |
| Legacy host asset read | Полный canonical аллоцируется до GUI cap, может повторяться на чанках; будущий bounded transfer owner устраняет риск. |
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
