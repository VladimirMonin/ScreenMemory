---
applyTo: "crates/gui/**,apps/gui/**,scripts/preview-gui.ps1,scripts/capture-preview-window.ps1,scripts/test-foundation-smoke.ps1,scripts/test-preview-rejection.ps1,scripts/preview-common.ps1,scripts/verify.ps1,scripts/test-verify.ps1,docs/progress/S01/t06d-gui.*"
name: "APP.Gui"
description: "Читай при работе с окном Tauri+React, его диагностикой и галереей S02, командами, --self-check и developer preview окна."
---

# APP — диагностическое окно и первая галерея S02

Источник: S01 §§10.1, 10.3, 5.3; [ADR-010](../docs/adr/010-diagnostic-window.md). `crates/gui`
(package `sm-gui`, binary `screenmemory-gui`) — единственный Tauri-потребитель. Окно не
говорит по wire, не открывает SQLite, не линкует storage owner и не создаёт второй host:
каждый handler — тонкий typed adapter над одним `GuiSession`, который использует **тот же**
`sm_session::ApplicationClient`, что `shot` (shared primitives `sm-cli`
`connect_session`/`ClientFailure`/`config_message`/`target_failure`). Handler не принимает
library root, host image, frame, credential или method и не может остановить host.
S03 handlers принимают только typed source/destination roles: ImportRequest,
ImportControlRequest/ReportPageRequest, ExportRequest и RecordMetadataRequest.
Root/worker authority остаётся у host; renderer paths не используются для IO в GUI.

Конфиг окна — только явный абсолютный локальный `--config <file>` со схемой dev-preview
CLI: `schema_version = 1`, `environment = test`, абсолютный `root`, canonical UUID
`library_id`, абсолютный `host_executable`. Никаких default path, CWD/ENV/PATH/HOME
fallback и UNC/device/ADS spelling; отказ — safe message и exit 2 **до** создания webview.
Production first-run вне среза.

Состояния (§10.1): `connecting` (одна bounded попытка, `bounded_ms` из shared констант),
`ready`, `runtime_unavailable` (AI включён, bridge не ready — это не отказ всего
приложения), `protocol_mismatch` (major/capability/format, без угадывания ответа),
`root_credential_error`, `host_not_running`. `state_for_failure` — тотальный маппинг
опубликованных `ErrorCode`; выключенная AI policy остаётся `ready`. Envelope не содержит
count/path/model полей: незавершённые возможности перечисляются как «не реализовано на этом
этапе» из owner-списка `NOT_IMPLEMENTED`, а не показываются пустым «0 изображений».
Закрытие окна закрывает только это соединение: host продолжает работать, повторное окно
подключается к тому же `host_instance_id`.

S03: `gui_library_page` получает source-aware committed records, по 24 с opaque cursor;
legacy `gui_library_recent` остаётся первой страницей. Imported rows не превращаются
в CaptureContext. Empty list не расходует первый auto-preview; новые Saved обновляют
список, а generation guards не дают старому ответу сменить позднюю selection. `gui_asset_preview` принимает **только**
канонический lowercase UUIDv4 `assetId`, без path/URL/transport; host проверяет наличие
asset в этой библиотеке. `read_asset_full` вызывает `library.asset.preview`: native file lane выдаёт WebP
из сохранного канона (AVIF/large canonical → contained derivative max-side2048),
не требует исходного файла. ≤8 MiB transfer отказывает по `PREVIEW_TOO_LARGE`
при превышении объявленного размера **до** накопления hex, сверяет
asset_id, offset, next_offset, total_size, WebP MIME, SHA256 по всем чанкам и EOF;
прочие ошибки дают `PREVIEW_UNAVAILABLE` или безопасный host code. Канон при отказе
не изменяется. React превращает только проверенный bounded hex в Blob URL `image/webp`,
отзывает URL при смене/закрытии/unmount, не использует `file://` или сеть. Список
обновляется на ready, после Saved и по кнопке. При первом успешном списке первый
asset с canonical UUIDv4 и объявленным размером от 1 байта до 8 MiB открывается
автоматически один раз; обновление списка не переоткрывает preview. Pagination доступна кнопкой, gallery имеет bounded scroll. Thumbnails, pin/editor,
clipboard и полная S07 остаются отдельными задачами. Preview host cache: ≤4 immutable
buffers ×8 MiB, 120s; eviction/expiry дают отказ offset, не подмену transfer. Legacy
canonical read сохраняет свой checksum, но использует fixed scratch и chunk buffer,
не full-file allocation на каждый chunk.
Native Drop передаёт только Unicode local paths через fixed `gui-import-paths` event;
React listener освобождается даже при async registration после StrictMode cleanup.
Импорт начинается явным подтверждением, не mount effect; batch UUID сохраняется
до invoke и между reopen. Pause/cancel/resume/retry используют тот же batch, export
retry — тот же сохранённый request/operation UUID. Metadata требует отдельный
ReadSourceMetadata scope. Genuine drag/input acceptance — отдельная live проверка.

Frontend (`apps/gui`, Vite+React+TS) импортирует только React, `@tauri-apps/api/core`,
локальные модули и **типы** сгенерированного контракта; SDK LM Studio, SQLite, native
capture binding, node builtins (кроме test-файлов), node net/child_process и собственный
транспорт запрещены; `apps/gui/scripts/check-boundaries.mjs` сверяет вызовы с реально
зарегистрированными командами `crates/gui/src/shell.rs`, а `check-parity.mjs` — версии
сборки/протокола/envelope с Rust-владельцами. «Открыть диагностический отчёт» открывает
только валидированный cache/log каталог из конфига (namespace resolver + отказ на
link/reparse); `explorer.exe` берётся из `GetWindowsDirectoryW`, не из PATH.

`--self-check` — headless connect+status+doctor против owned host: ровно один JSON в stdout,
exit-коды как у CLI (0/2/4/6/8), безопасная строка в stderr. Offline verification **не
запускает окно**: `verify.ps1` собирает frontend (assets встраиваются в crate), гоняет
`cargo test -p sm-gui`, frontend typecheck/lint/test и помечает `GUI-WINDOW-RUNTIME` как
NOT_RUN. Реальное окно запускает только `scripts/preview-gui.ps1` (owned
`.pi/preview/<uuid>` root + явный конфиг + уникальный build-log): `-Action selfcheck`
(по умолчанию) или `-Action window`. Wrapper сохраняет immutable `result.json` и
raw exits в новом `.pi/temporary/<uuidv4>`; один JSON stdout — контракт binary,
не прогресс-вывода PowerShell wrapper. Окно закрывается агентом автоматически через
`-WindowHoldSeconds` (default 2s), без просьбы владельцу кликнуть. `finally` закрывает
только собственный window handle. Stop/credential/root finalization разрешены лишь
после полного `Resolve-ValidatedPreview` **до первого connect**: marker/tree + независимый
create-only init-issued directory/library binding + config root/schema/test/image.
Validated tuple/config+binding hashes замораживаются. Отказ preflight не разрешает
cleanup; authenticated B не доказывает A→B. Mutation после arm запрещает privileged
cleanup и сохраняет root/credential, возможный host явно локальный BLOCKED; окно
закрывается через own handle. При неизменных входах ошибки connect/window сохраняют
обычный stop→absence→remove; `-Cleanup` удаляет только свой exact Test credential/root.
Контракт binding/legacy и ограничение same-user TOCTOU — у [HOST.Composition](HOST.composition.instructions.md).
`scripts/capture-preview-window.ps1` подтверждает marker/config/process/image/HWND
owner и снимает только PrintWindow этого HWND с per-monitor DPI-awareness; не
читает desktop/clipboard. Кадр и его physical размеры/hash остаются private evidence.
`scripts/test-foundation-smoke.ps1` проверяет selfcheck/window/reopen и два реальных
injected failures после connect/spawn: raw 1, подтверждённая финализация, и отдельную
12-case two-root/mutation suite. Mutation fixtures `mutate-config`/`mutate-binding`
проверяют hash отказ даже при семантически неизменном JSON; harness восстанавливает
только собственные inputs, завершает свои processes и удаляет свои exact keys.
Это не capture→Saved→preview acceptance.
Workspace allowlists (`verification-common.ps1`,
`tests/contracts/check_boundaries.py`, `packages/protocol/scripts/check-boundaries.mjs`)
перечисляют ровно `packages/protocol,apps/gui` — новый пакет требует явного обновления, не
wildcard. Toolchain только project-local (pinned Rust/Node/pnpm, cargo cache, локальный
`node_modules`); WebView2/CMake/сеть не подменяются обходным путём, а сообщаются как BLOCKED.
