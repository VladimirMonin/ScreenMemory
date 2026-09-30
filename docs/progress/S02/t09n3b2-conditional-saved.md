# S02-T09n3b2 — условный storage-only pending → Saved

Baseline: clean HEAD `20b3972`; только storage, boundary-check и инструкция. Исходные девять планов, `images::PublishedFile`, host/startup/clipboard и миграции не изменены. Независимая приёмка/commit — у интегратора; здесь нет stage/commit.

**Условная модель угроз:** не предполагается hostile same-user процесс с заранее полученными writable mappings/views, admin/kernel или raw-volume capability. Безусловный инвариант `pending→Saved` остаётся **BLOCKED**: write-denying HANDLE отказывает обычному writer, но не доказывает исключение всех заранее созданных writable views. Это не файлово-SQL атомарная транзакция и не sandbox. Process-kill, power-loss/NTFS durability и общий RSS ≤1 GiB **NOT_RUN/NOT_PROVEN**; host/retry/startup/clipboard **NOT_WIRED**. Только synthetic test fixtures, не живые пользовательские данные.

`StorageWorker::commit_sealed_recovery(SealedPublished)` потребляет непродлеваемый private non-Clone proof: file lane повторно сравнивает bytes/length/SHA на pinned sealed HANDLE до `BEGIN IMMEDIATE`; затем SQLite worker владеет seal сквозь COMMIT/rollback и не делает FS/codec IO внутри SQL. В транзакции Saved-first проверяет raw principal/mode до проекции receipt, полный original fingerprint/metadata и возвращает только исходный receipt; чужое/изменённое — без metadata. Иначе scoped trusted read сверяет **полный** pending DTO с seal witness, включая IDs/frame, request trigger/fingerprint, геометрию, pixel/context SHA, ordered warnings, profile, captured UTC и SQLite planned UTC. Удаление pending (ровно одна строка) предшествует INSERT records, assets, saved journal в той же транзакции под v4 triggers. Идентификаторы и месяц пути берутся из validated DB intent; automatic provenance — из context+capture time, encoded SHA/length — из verified candidate; `updated_at_utc` — SQLite UTC. Commit с неопределённым результатом не выдаёт Saved без повторно прочитанной matching saved row на том же SQLite worker; seal удерживается до завершения.

Synthetic `s02_t09n3b2_01/02` (UUIDv4 roots): F06 после публикации до BEGIN, F07 после SQL inserts до COMMIT дают 0 partial rows, сохранённые stage manifest/final и pending. Restart/reacquire повторно открывает существующий final, успешный retry пишет по одной точной record/asset/journal с planned месяцем и исходными warnings/IDs/bytes; ещё один restart подтверждает replay. Один alpha=0 pixel с ненулевым RGB проходит настоящий exact WebP decode. Second pre-commit seal проходит Saved-first replay; foreign principal/mode (pending и journal), missing pending, изменённые warnings/planned/context, tampered final и v4 trigger collision отвергаются; восстановление оригинальных bytes позволяет retry. F06/F07 здесь **детерминированные in-process failpoints**, не process-kill.

Проверки на конечных байтах: Windows x64, offline pinned `.pi/t09a-run.cmd` с локальным MSVC; raw exits:

| Команда | Exit | Результат |
|---|---:|---|
| `.pi/t09a-run.cmd test --offline --locked -p sm-storage --lib s02_t09n3b2 -- --test-threads=1` | 0 | 2/2 synthetic, final после последней правки |
| `.pi/t09a-run.cmd test --offline --locked -p sm-storage -p sm-isolation -p sm-codec --quiet -- --test-threads=1` | 0 | final bytes: storage 113, native isolation/codec, doc compile-fail |
| `.pi/t09a-run.cmd test --workspace --offline --locked --quiet -- --test-threads=1` | 0 | final bytes: serialized workspace, 1 existing ignored |
| `.pi/t09a-run.cmd clippy --workspace --all-targets --offline --locked -- -D warnings` | 0 | after final source edit |
| `.pi/t09a-run.cmd fmt -p sm-storage -- --check` | 0 | scoped storage |
| `python tests/contracts/check_boundaries.py` | 0 | 15 crate boundaries; выполнено до последней source edit, повторено ниже |
| `powershell.exe -NoProfile -NonInteractive -File scripts/verify-contracts.ps1` | 0 | implemented subset PASS, S01 IN_PROGRESS; повторено ниже |
| `git diff --check`; `git diff --cached --name-only` | 0; 0 | whitespace clean; no staged files |

Обычный full workspace fmt имеет известный прежний drift в незатронутых пакетах, поэтому для этого среза проверяется scoped `sm-storage` fmt. Недоступен Serena language server (`Active language servers: []`), актуальные source/graph подтверждены чтением и runtime тестами; Codebase Memory и ast-grep применены для владельцев и SQL паттернов.

## Follow-up — исправление независимого review FAIL

Независимый reviewer обнаружил три непроверенных поля на Saved-first replay. В **приватном** `recovery_commit::saved_replay` после scoped principal/raw mode precheck дополнительная связанная SQL-проекция `io/r/a` сверяет `a.asset_type=original`, `r.created_at` с sealed planned time и `r.library_id` с trusted sealed witness. Отсутствующая связанная строка и любые несовпадения дают только `IntegrityFailed`, без receipt metadata; `images::saved_operation` и legacy saved route не менялись. Файлового IO внутри SQLite-транзакции не добавлено.

Новый synthetic `s02_t09n3b2_03` независимо меняет каждое поле **после** первого Saved при заранее полученном втором seal: `original→preview`, валидное иное `records.created_at`, затем в отдельном fixture valid-FK `library_meta.library_id` вместе с `records.library_id` через deferred FK (singleton schema не допускает вторую library row). Все три случая обязаны дать `IntegrityFailed`; pending=0 и ровно по одному record/asset/journal остаются, stage и final не меняются. Перед исправлением первый сценарий дал фактический **FAIL**, raw exit 101: `Ok(Saved)` вместо `IntegrityFailed`; предшествующий тестовый panic при открытом fixture дал exit 127, устранён закрытием worker перед assertion. Это ожидаемая красная регрессия, не результат итоговых байтов.

Проверки на исправленных байтах (Windows x64, pinned offline runner, raw exits):

| Команда | Exit | Результат |
|---|---:|---|
| `.pi/t09a-run.cmd test --offline --locked -p sm-storage --lib s02_t09n3b2_03 -- --test-threads=1` | 0 | 1/1; все три synthetic SQL mutation |
| `.pi/t09a-run.cmd test --offline --locked -p sm-storage --lib s02_t09n3b2 -- --test-threads=1` | 0 | 3/3; F06/F07 и replay без регрессии |
| `.pi/t09a-run.cmd test --workspace --offline --locked --quiet -- --test-threads=1` | 0 | serialized workspace, storage 114/114; 1 existing ignored |
| `.pi/t09a-run.cmd clippy --workspace --all-targets --offline --locked -- -D warnings` | 0 | без предупреждений |
| `.pi/t09a-run.cmd fmt -p sm-storage -- --check` | 0 | scoped fmt |
| `python tests/contracts/check_boundaries.py` | 0 | 15 crate boundaries, проверка новой private projection; до корректировки boundary literal после fmt был exit 1, исправлен |
| `powershell.exe -NoProfile -NonInteractive -File scripts/verify-contracts.ps1` | 0 | implemented subset PASS; S01 IN_PROGRESS |
| `git diff --check`; `git diff --cached --name-only` | 0; 0 | whitespace clean; ничего не staged |

Review после этого исправления ещё не получен, commit/stage не выполнялись. Условная модель угроз, безусловный **BLOCKED** и все прежние **NOT_WIRED/NOT_RUN/NOT_PROVEN** пределы остаются без изменения.

## Второй независимый review FAIL — дополнительная узкая коррекция

Повторный reviewer нашёл ещё два Saved-first пробела. После прежней проверки principal и сырого mode, **до** чтения private saved metadata и выдачи Saved, `saved_replay` теперь проверяет текущий `library_meta.library_id` против trusted sealed witness. Его private `io/r/a` проекция дополнена `io.created_at_utc` с точным сравнением с sealed `planned_created_at_utc` (`expected.created_at`). Проверки `original`, `records.created_at` и `records.library_id` из прошлого follow-up сохранены; public `images::saved_operation`/legacy route не меняются, файлового IO внутри tx нет. Повреждение выдаёт только `IntegrityFailed` без receipt.

Независимые synthetic `s02_t09n3b2_04/05` получают два seal до первого Saved, затем меняют только журнал `created_at_utc` на валидное иное время либо только singleton `library_meta.library_id` через **отдельное** соединение с `foreign_keys=OFF` (явно имитированная corruption, `records.library_id` остаётся исходным). По обоим путям до исправления получался ложный `Ok(Saved)` — два отдельных запуска с raw exit **101**. После исправления оба дают `IntegrityFailed`; pending=0, ровно по одному record/asset/journal, canonical и stage без изменений. Старый тест `03` с valid-FK сменой library_meta **и** records проверяет независимый предыдущий инвариант. Boundary guard дополнен проверкой порядка owner/mode → singleton и двух новых сравнений.

Проверки на конечных байтах, pinned offline Windows x64 (raw exit):

| Команда | Exit | Результат |
|---|---:|---|
| `.pi/t09a-run.cmd test --offline --locked -p sm-storage --lib s02_t09n3b2 -- --test-threads=1` | 0 | 5/5, включая оба новых corruption tests и три прежних |
| `.pi/t09a-run.cmd test --workspace --offline --locked --quiet -- --test-threads=1` | 0 | serialized workspace, storage 116/116; один existing ignored |
| `.pi/t09a-run.cmd clippy --workspace --all-targets --offline --locked -- -D warnings` | 0 | без предупреждений |
| `.pi/t09a-run.cmd fmt -p sm-storage -- --check` | 0 | scoped fmt |
| `python tests/contracts/check_boundaries.py` | 0 | пятнадцать crate boundaries и private guard |
| `powershell.exe -NoProfile -NonInteractive -File scripts/verify-contracts.ps1` | 0 | implemented subset PASS; S01 IN_PROGRESS |
| `git diff --check`; `git diff --cached --name-only` | 0; 0 | чистый diff, staging пуст |

Независимое повторное принятие этой коррекции ещё не получено; безусловный инвариант **BLOCKED**, условная модель угроз и все прежние **NOT_RUN/NOT_PROVEN/NOT_WIRED** ограничения остаются неизменными. Git stage/commit не выполнялись.
