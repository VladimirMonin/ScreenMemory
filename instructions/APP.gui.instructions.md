---
applyTo: "crates/gui/**,apps/gui/**,scripts/preview-gui.ps1,scripts/preview-common.ps1,scripts/verify.ps1,scripts/test-verify.ps1,docs/progress/S01/t06d-gui.*"
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
path, host image, frame, credential или method и не может остановить host.

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

S02: `gui_library_recent` получает от authenticated `GuiSession::list` только первую
фиксированную страницу из 24 committed records. `gui_asset_preview` принимает **только**
канонический lowercase UUIDv4 `assetId`, без path/URL/transport; host проверяет наличие
asset в этой библиотеке. Legacy `read_asset_full` для этого окна отказывает по
`PREVIEW_TOO_LARGE` при объявленном размере >8 MiB **до** накопления hex, сверяет
asset_id, offset, next_offset, total_size, WebP MIME, SHA256 по всем чанкам и EOF;
прочие ошибки дают `PREVIEW_UNAVAILABLE` или безопасный host code. Канон при отказе
не изменяется. React превращает только проверенный bounded hex в Blob URL `image/webp`,
отзывает URL при смене/закрытии/unmount, не использует `file://` или сеть. Список
обновляется на ready, после Saved и по кнопке; pagination, thumbnails, pin/editor,
clipboard и пользовательское окно остаются отдельными задачами, а не обещанием этой
галереи. Legacy host read реализует полную проверку canonical за один вызов и может
аллоцировать полный файл до отказа GUI по размеру: это остаточный риск host-owner.

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
(по умолчанию) или `-Action window`. Workspace allowlists (`verification-common.ps1`,
`tests/contracts/check_boundaries.py`, `packages/protocol/scripts/check-boundaries.mjs`)
перечисляют ровно `packages/protocol,apps/gui` — новый пакет требует явного обновления, не
wildcard. Toolchain только project-local (pinned Rust/Node/pnpm, cargo cache, локальный
`node_modules`); WebView2/CMake/сеть не подменяются обходным путём, а сообщаются как BLOCKED.
