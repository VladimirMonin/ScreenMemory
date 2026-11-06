# ScreenMemory

Локальная библиотека изображений для Windows: быстро сохраняет скриншоты и
импортированные картинки, хранит собственную копию, по разрешению дополняет их
AI-описанием и подписями, ищет по тексту (SQLite FTS5) и отдаёт доступ человеку
через окно, а терминалу и агентам — через CLI и MCP.

> **Статус: в разработке, релиза нет.** Репозиторий публикуется как открытый
> дневник разработки. Здесь есть работающий Rust-workspace, host/IPC, native capture,
> WebP-хранение, импорт/экспорт, SDK-мост и очередь, ручные карточки и FTS5-поиск —
> но **ни один этап не принят целиком и нет установщика**. Подробный статус каждого
> этапа — в [docs/TASKS.md](docs/TASKS.md).

## Что уже реализовано

| Этап | Срез | Состояние |
|---|---|---|
| S01 Foundation | один host/root/DB/IPC-клиент, изоляция, observability | IN_PROGRESS, не принят целиком |
| S02 Capture / Library | freeze → Saved, режимы, recovery, видимый preview | IN_PROGRESS |
| S03 Import / Images / Export | импорт, WebP/AVIF, metadata, resize, экспорт | ACCEPT_CODE_SCOPED |
| S04 Processing / LM Studio | durable очередь, ModelManager, SDK вне WebView | ACCEPT_CODE_SCOPED |
| S05 Description / Editing | ручные карточки, категории, Keep/Inherit/Set, replay | ACCEPT_CODE_SCOPED (manual-core) |
| S06 Index / Search | FTS5, фильтры, общий поиск CLI/GUI, cancel | CODE_CHECKPOINT_SCOPED |
| S07 GUI / CLI / MCP | агентский доступ, scopes, полный GUI/MCP | IN_PROGRESS (частичный checkpoint) |
| S08 Release / Operations | backup/restore/purge, NSIS, upgrade | PENDING |

Отсутствуют: OCR, vision-описания, embeddings/семантический поиск (исключены
[ADR-011](docs/adr/011-private-release-scope.md)), полный MCP с выдачей пикселей,
установщик и приёмочные прогоны на «живом» вводе.

## Структура

```
crates/      Rust workspace: domain, protocol, application, storage, host,
             session, pipe, framing, capture, codec, image, image-worker,
             runtime, credentials, isolation, observability, cli, gui
apps/gui/    React/TypeScript диагностическое окно (Vite)
packages/    @screenmemory/protocol (TS-клиент и схемы) и lmstudio-bridge
schemas/     JSON Schema протокола application/1.0
docs/        план v3, восемь планов этапов, ADR, evidence-отчёты, доска задач
instructions/ правила подсистем для разработки
scripts/     offline-верификация и preview на PowerShell
```

Архитектурные границы в двух словах: один `shotd` — владелец библиотеки, capture и
очередей; GUI, CLI и MCP — клиенты общих сервисов по одному типизированному протоколу;
приватный runtime LM Studio работает вне WebView; сохранение и поиск работают без AI.

## Сборка и проверки

Предварительно должны быть установлены (репозиторий ничего не скачивает):

- Rust 1.98.1 (MSVC, `x86_64-pc-windows-msvc`) — см. [rust-toolchain.toml](rust-toolchain.toml)
- Node 24.13.0 и pnpm 10.32.1 — см. [package.json](package.json)
- Python 3.14 с `jsonschema` — для проверок контрактов
- CMake — для сборки bundled WebP
- MSVC build tools и WebView2

```powershell
cargo check --offline --locked
cargo test --workspace --offline --locked
powershell.exe -NoProfile -NonInteractive -File scripts/verify.ps1
powershell.exe -NoProfile -NonInteractive -File scripts/verify-contracts.ps1
pnpm typecheck; pnpm lint; pnpm test
```

`scripts/` намеренно работает офлайн и никогда не поднимает продуктовый host.
Живой захват рабочего стола в автотестах запрещён — только изолированный стенд
(см. [scripts/README.md](scripts/README.md)).

## Документация

- [Навигатор](docs/README.md) · [Доска задач](docs/TASKS.md)
- [Master-план v3](docs/ScreenMemory_Full_Development_Plan_v3.md) — требования и scope V1
- Планы этапов: [S01](docs/ScreenMemory_S01_Foundation_Architecture.md),
  [S02](docs/ScreenMemory_S02_Capture_Library.md),
  [S03](docs/ScreenMemory_S03_Import_Images_Export.md),
  [S04](docs/ScreenMemory_S04_Processing_LMStudio.md),
  [S05](docs/ScreenMemory_S05_Description_Editing.md),
  [S06](docs/ScreenMemory_S06_Index_Search.md),
  [S07](docs/ScreenMemory_S07_GUI_CLI_MCP.md),
  [S08](docs/ScreenMemory_S08_Release_Operations.md)
- Решения: [ADR-001 донор](docs/adr/001-upstream-strategy.md),
  [ADR-011 scope](docs/adr/011-private-release-scope.md),
  [ADR-012 агентский доступ](docs/adr/012-agent-access-core.md)
- Отчёты исполнения: [docs/progress/](docs/progress/)

## Лицензия

[MIT](LICENSE). Сторонние компоненты и атрибуция донора —
[THIRD-PARTY-NOTICES.md](THIRD-PARTY-NOTICES.md).
