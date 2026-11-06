# AGENTS.md — ScreenMemory

Краткий маршрут для coding-агентов. Общение и отчёты — по-русски.
Подробные правила хранятся в корневой папке `instructions/`, не дублируются здесь.

**Это частное локальное приложение для одного владельца, не коммерческий продукт.**
Сначала доводи до работающего пользовательского сценария (захват → библиотека → видимый
preview); соизмеряй глубину инженерной проверки с реальным риском и не застревай в
бесконечных подготовительных срезах. Незакрытый риск честно помечай `NOT_RUN`/`BLOCKED`
и оставляй следующим этапом; не выдавай его за PASS. При этом не трогай чужие/живые
данные и не ослабляй уже принятые контракты сохранности.

## Начало работы и источники истины

1. Прочитай [доску текущих задач](docs/TASKS.md), [навигатор](docs/README.md),
   [принятое уточнение scope](docs/adr/011-private-release-scope.md), релевантные
   разделы [master v3](docs/ScreenMemory_Full_Development_Plan_v3.md) и план этапа.
2. Проверь фактическое дерево, Git/worktree и чужие изменения до первой записи.
   Исходные девять планов неизменны; фактические manifests/исходники и статус срезов
   сверяй с `docs/progress/S01/`, а не с историческим отсутствием кода/Git.
   Не выдавай спецификацию или mock за реализованное и проверенное приложение.
3. Master владеет scope V1 и общими инвариантами; S01–S08 — детализацией своих этапов;
   код, manifests и evidence после их появления — фактическим состоянием.
4. Планы связаны SHA-256. Не перемещай и не переписывай их попутно; изменения общих
   контрактов согласуй через change note/ADR и обновление затронутых ссылок/хешей.
   Поздние `CN-Sxx-*` не означают автоматически принятый или реализованный контракт.

## Инструкции: читать и поддерживать

Перед изменениями прочитай **все** инструкции, относящиеся к затронутым владельцам.

| Инструкция | Когда читать |
|---|---|
| [DOCS.instructions_style](instructions/DOCS.instructions_style.instructions.md) | Создание, обновление, переименование или разделение инструкций и AGENTS.md |
| [DOCS.commit_messages](instructions/DOCS.commit_messages.instructions.md) | Группировка изменений, staging, проверки и создание commit |
| [CORE.code_intelligence](instructions/CORE.code_intelligence.instructions.md) | Поиск реализации, символов, связей, структурных паттернов и оценка влияния |
| [WORKFLOW.release](instructions/WORKFLOW.release.instructions.md) | Продолжение S01–S08, Markdown-доска, Pi Sol6.1/Astra6, FTS5-only, SDK в поставке и агентская приёмка |
| [APP.session](instructions/APP.session.instructions.md) | Authenticated RPC session, shared Rust client, authorization, bounded dispatch/cancellation |
| [HOST.composition](instructions/HOST.composition.instructions.md) | Реальные shotd/shot, bootstrap/shutdown-порядок, trusted preview configuration, CLI JSON/exit codes, autostart и developer preview |
| [APP.gui](instructions/APP.gui.instructions.md) | Диагностическое окно Tauri+React, его команды/конфиг, `--self-check` и developer preview окна |
| [BUILD.protocol_contracts](instructions/BUILD.protocol_contracts.instructions.md) | Изменение Rust DTO/ports, схем, fixtures и проверка совместимости протокола |
| [BUILD.verification](instructions/BUILD.verification.instructions.md) | Offline verify, Windows runner, шаги и synthetic verification fixtures |
| [BUILD.framing](instructions/BUILD.framing.instructions.md) | Bounded async framing, shared buffer budget, deadline и writer poison |
| [INFRA.observability](instructions/INFRA.observability.instructions.md) | SafeEvent, bounded logger, sinks, rotation и S01-LOG проверки |
| [INFRA.root_isolation](instructions/INFRA.root_isolation.instructions.md) | Identity продукта, explicit root, Windows native root resolver и pre-host isolation |
| [DATA.storage_owner](instructions/DATA.storage_owner.instructions.md) | Foundation SQLite worker, миграции, DB entries и владение HostLease |
| [DATA.knowledge](instructions/DATA.knowledge.instructions.md) | Manual карточка, Keep/Inherit/Set, категории, replay/revisions, sensitivity и effective source/hook S06 |
| [DATA.search](instructions/DATA.search.instructions.md) | FTS5/exact source projection, shared поиск/filters/readers/sessions/cancel и production gold/10k |
| [SEC.credentials](instructions/SEC.credentials.instructions.md) | Локальные/session/backend credentials, bounded stdin, exact Windows Credential Manager authority и host scopes |
| [SEC.agent_access](instructions/SEC.agent_access.instructions.md) | Persistent metadata-only agent profiles/connections, exact enrollment/revoke, current authority и security-bound cursors |
| [SEC.pipe_endpoint](instructions/SEC.pipe_endpoint.instructions.md) | Windows named-pipe ACL, sealed pool, peer identity и trusted server image |
| [CAPTURE.native](instructions/CAPTURE.native.instructions.md) | Win32 capture, region picker, overlay, hotkey, GDI/DIB |
| [CODEC.webp](instructions/CODEC.webp.instructions.md) | WebP codec (bundled libwebp), exact lossless, bounds/отказы |
| [IMAGE.pipeline](instructions/IMAGE.pipeline.instructions.md) | S03 ingest/source snapshots/decode/metadata/color/resize/export и private worker/packaging boundary |
| [RUNTIME.sdk](instructions/RUNTIME.sdk.instructions.md) | Published SDK bridge, host execution/ModelManager, private runtime/supervisor, load intent/instance ownership, inference lease, cancellation и staging |

- **Каждую новую инструкцию обязательно добавляй сюда рабочей ссылкой с условием чтения.**
  При переименовании, разделении или удалении обновляй ссылки и маршруты.
- **По мере необходимости обновляй существующие или добавляй новые инструкции**,
  когда меняются устойчивые owners, контракты, workflow, команды или ограничения.
  Делай это в том же логическом срезе по `DOCS.instructions_style`; не создавай
  дубликаты и не переписывай запреты ради удобства текущей задачи.
- **Держи AGENTS.md кратким:** правила подсистем и подробные команды выноси
  в тематические инструкции. Каталог ссылок может расти; повторение их содержания — нет.

## Работа с кодом

**Используй Codebase Memory, Serena и ast-grep по назначению**, согласно
`CORE.code_intelligence`: Codebase — владельцы и связи, Serena — символы,
декларации и references, ast-grep — структурные паттерны вызовов.
Перед чтением целого source-файла начни с Serena; перед изменением поведения
проверь references. Критичные результаты индекса сверяй с актуальным worktree.
Для известных документов/manifests используй прямое чтение; runtime доказывают
тесты/trace, не статический граф. Недоступность инструмента отмечай явно.

## Ключевые границы продукта

Целевой стек: Rust, Tauri + React/TypeScript, SQLite/FTS5, LM Studio SDK в поставке.
Windows x64 — первая платформа; V1 включает S01–S08 с уточнением ADR-011:
**без embeddings/sqlite-vec/семантического поиска**, vision/OCR остаются.

- Один пользовательский `shotd` — writer/owner библиотеки, capture и очереди.
  GUI/CLI/MCP — клиенты общих services; приватный SDK bridge работает вне WebView.
- Freeze раньше UI/focus; после выделения второго capture нет. `Saved` — только
  после проверенного канона и commit БД. Канон и принятый staging — не кеш.
- WebP/AVIF, полный канон, независимость сохранения от AI, отдельные Manual/Off/OCR
  policies, приоритет ручных overrides и revision guards нельзя упрощать молча.
  FTS5-запрос не запускает модель; SDK и приватный runtime работают вне WebView.
- Не трогай чужие модели, внешние исходники и живые данные. Тесты — в изолированных
  roots на синтетике; migration/cleanup требуют preflight. Нет скрытой передачи
  содержимого, скачивания моделей или чувствительных данных в обычных логах.
- Inspector — донор кода: сохрани notices и отдели identity/data/updater до запуска.

## Исполнение и отчёт

Работай связными продуктовыми milestones, без дробления на микрозадачи:
контракт → реализация → реальные failure/cancel/race и live проверки → review →
evidence → commit. Не включай чужие правки или несвязанный рефакторинг.
Проверки выбирай по реальным manifests; плановые Cargo/pnpm/scripts, которых ещё нет,
не выдавай за исполненные. Сохраняй raw exit codes;
`NOT_RUN`/`BLOCKED` не равны `PASS`, code commit не равен принятому этапу.

**Схема:** native Pi `openai-codex/gpt-6.1-sol` пишет и независимо проверяет;
при тупике — native Pi oracle `openai-codex/gpt-6-astra`. Parent оркестрирует,
оценивает визуал/поиск, принимает и коммитит. Пользователя не назначать ручным тестером.
Workflow: `.pi/orchestration/screenmemory.milestone.workflow.js` (явные writerModel/reviewModel).
Блокер локализуй, продолжай независимую работу; не меняй provider/режим скрыто.

Делегирование — только при разрешении пользователя/применимых инструкций;
один writer на worktree, независимый reviewer — read-only, commits — у интегратора.
Push, публикация и опасные Git-операции требуют отдельного явного разрешения
по инструкции коммитов.

В конце: изменённые пути, реальные проверки, пропуски/блокеры и следующий шаг;
SHA — только если commit действительно создан. Подготовка плана не запускает реализацию.
