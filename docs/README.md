# ScreenMemory — документы и план старта

## Фактическое состояние

При первичной вычитке 27 сентября 2026 года в проекте обнаружены **master v3 и все
восемь детальных планов**, но нет исходников приложения, build manifests, тестов,
Git-репозитория или отчётов исполнения. Разработка S01 не считается начатой по факту
наличия её спецификации. Новые организационные правила — в [AGENTS.md](../AGENTS.md)
и корневой папке `instructions/`.

Это навигатор и предложение порядка работы, не девятый план этапа и не разрешение
на автоматический запуск реализации, скачивание моделей или публикацию.
При продолжении сначала сверяй фактическое дерево и появившиеся отчёты.

## Карта планов

| Документ | Область и ожидаемый результат |
|---|---|
| [Master v3](ScreenMemory_Full_Development_Plan_v3.md) | Scope V1, requirements, архитектура, общие контракты и Definition of Done |
| [S01 — Foundation](ScreenMemory_S01_Foundation_Architecture.md) | Inspector baseline, изоляция, Windows toolchain, один host/IPC, SDK/native proofs → `foundation-alpha` |
| [S02 — Capture / Library](ScreenMemory_S02_Capture_Library.md) | Freeze до UI, четыре режима, immutable assets, durable save/recovery → `capture-library-alpha` |
| [S03 — Import / Images / Export](ScreenMemory_S03_Import_Images_Export.md) | Файлы/папки, WebP/AVIF, metadata, resize и экспорт → `image-library-alpha` |
| [S04 — Processing / LM Studio](ScreenMemory_S04_Processing_LMStudio.md) | Durable jobs, политики, SDK bridge, профили, ownership и TTL → `runtime-processing-alpha` |
| [S05 — Description / Editing](ScreenMemory_S05_Description_Editing.md) | OCR, AI-карточки, категории, overrides, безопасная публикация → `knowledge-alpha` |
| [S06 — Index / Search](ScreenMemory_S06_Index_Search.md) | FTS5, Qwen embeddings, sqlite-vec, фильтры, revisions/generations → `search-beta` |
| [S07 — GUI / CLI / MCP](ScreenMemory_S07_GUI_CLI_MCP.md) | Полные интерфейсы, scopes, выдача pixels/resources и parity → `feature-complete-beta` |
| [S08 — Release / Operations](ScreenMemory_S08_Release_Operations.md) | Backup/restore/purge, Windows NSIS, upgrade, clean-machine gates → Windows V1 либо RC с блокерами |

Планы читаются по текущей задаче и зависимостям. Перечень будущих файлов/crates,
команд `shot` и `scripts/verify*.ps1` внутри планов не подтверждает их существование.
Фактические отчёты появятся при исполнении в `docs/progress/Sxx/`.

## Результат первичной вычитки

**Прочитано:** master целиком; в S01 — вводная, baseline/toolchain, исполняемые задачи,
приёмка и трассируемость; в S02–S08 — структура, scope, входные обязательства и
разделы изменений общих контрактов. Это не построчный технический аудит всех восьми
детализаций и не повторная проверка их внешних источников, SQL или SDK API.

1. **Устарела вводная master:** она говорит, что детализации ещё не написаны;
   на диске присутствуют все S01–S08. `docs/plans/00_Master_v3.md` — рекомендуемый
   там будущий путь, а не текущий. До согласованной ревизии пользуемся путями выше.
2. **Цепочка исходных документов согласована по байтам:** фактические SHA-256
   соответствуют значениям зависимостей в YAML. Исходные девять файлов сохранены
   без изменений; косметическая правка master тоже изменила бы его hash.
3. **Есть межэтапные уточнения, требующие решения до затронутой реализации:**
   - S05 §19.4, `CN-S05-01`: Windows OCR требует package identity; обязательство
     влияет на deployment/NSIS и должно быть учтено в раннем packaging decision.
   - S06 §19.9, `CN-S06-02`: фиксированное candidate window и новый search snapshot
     при расширении уточняют master, который допускает рост глубины на следующей странице.
   - S07 §19.3: новые MCP-профили, capture ticket, waiter exit130 и host bootstrap
     уточняют общие IPC/CLI/lifecycle contracts.
   - S08 §19.2: backup cut, новая authority при restore, двухфазный purge и
     OCR deployment требуют согласованного изменения контрактов владельцев.
   Эти записи не являются evidence уже внесённых или принятых изменений.
4. **Технические зависимости ещё нужно доказать:** сборка Inspector под MSVC,
   native capture, приватный Node/SDK, WebP/AVIF и SQLite/vec на Windows.
   Markdown, типы SDK и чтение документации такие gates не закрывают.

## Предложенный порядок работ

### 1. Подготовить согласованный вход в S01

- Зафиксировать корпус планов, его hashes и таблицу расхождений/change notes.
- Определить, какие уточнения принять, изменить или отложить; общие изменения
  вносить версионированно с затронутыми требованиями, ссылками и hashes.
- Согласовать размещение исходников Inspector и создание Git-истории **без потери
  текущих документов и происхождения донорского кода**. Не подменять этот шаг
  случайным `git init` с последующей безымянной копией всего upstream.

**Выход:** однозначная исходная спецификация, безопасный способ bootstrap и scope
первого среза. Это подготовка, не приёмка foundation.

### 2. Выполнить S01-T01–T02: donor audit и Windows baseline

Закрепить Inspector `fa314915273e2cb91006fa284e5dd48fa90aff3a`, сохранить notices,
составить source/dependency map. Проверить MSVC/SDK/Rust/Node/pnpm/WebView2 и
реальные build scripts. Сначала собрать baseline, затем отделить identity,
data/cache/secrets/updater и ненужные фоновые подсистемы до обычного запуска.
Runtime baseline до изоляции — только чистая тестовая Windows-среда.

**Выход:** воспроизводимый build либо конкретный blocker; карта заимствований,
environment manifest и результаты ограниченного menu capture smoke.

### 3. Выполнить S01-T03–T07: первый проверяемый сквозной срез

Создать только нужный workspace/domain/protocol skeleton; сразу добавить безопасные
логи, fixtures и fail-fast verification. Затем один host/root lock, минимальную
schema, named-pipe client, CLI `status/doctor`, маленький GUI и fake SDK bridge.
Не реализовывать полный поиск, AI-карточки и очередь всех этапов заранее.

**Выход:** GUI и CLI видят один host; CLI работает без GUI; fake bridge crash не
роняет host; намеренный subprocess exit7 делает verification красным.

### 4. Выполнить S01-T08–T10: снять дорогие технические риски

Проверить реальный published LM Studio SDK через приватный Node без React,
synthetic vision и Qwen embedding; отдельно — WebP exact, AVIF, FTS5 и sqlite-vec
на MSVC. Собрать staging и выполнить smoke без dev Node/Python/Rust в PATH.
Live-запуски и скачивания — только в согласованном окружении и scope.

**Выход:** runtime/native compatibility manifests и clean-package evidence.
Недоступный стенд или модель остаются `BLOCKED/NOT_RUN`, не «условно PASS».

### 5. Выполнить S01-T11–T12: принять основу, затем перейти к S02

Собрать regression/negative evidence, проверить G01–G10, провести review и
зафиксировать accepted code commit отдельно от commit отчёта. Непройденные
обязательные gates блокируют `ACCEPTED`, даже если часть кода уже написана.

**Выход:** `foundation-alpha` и handoff S02 либо честный отчёт с блокерами.
Следующая практическая цель — S02: полезный capture/save/reopen **без AI**.
Далее последовательно S03 → S04 → S05 → S06 → S07 → S08 по зависимостям таблицы.

**Рекомендуемое ближайшее поручение:** подготовка входного baseline и S01-T01,
без массового рефакторинга, запуска исходного Inspector на личных данных и
неразрешённых model downloads. Принятие всех контрактов поздних стадий не должно
блокировать безопасный read-only audit донора.
