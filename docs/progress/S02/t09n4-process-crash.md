# S02-T09n4 — bounded storage-only F06/F07 process crash

Исходная база: clean HEAD `b999010`; исходные девять планов не менялись. Это отдельный **test-only** process crash harness для *условного* `commit_sealed_recovery`, не host/shotd/clipboard и не startup retry. Нет обращения к живой библиотеке, процессам по имени/PID, пользовательскому экрану или моделям. Независимый review/acceptance и commit остаются у интегратора.

Parent создаёт новый project-owned `.pi/temporary/<UUIDv4>` root, pending intent и v2 staged triplet через существующие typed API, сохраняет manifest/context/pixels bytes+SHA; кладёт неизвестный sentinel `.tmp`, затем запускает собственный test executable через `std::process::Command` с точным `--exact ... --ignored` и уникальным UUIDv4 marker. Child только после проверки argv, root, phase и canonical lowercase UUIDv4 token получает lease, открывает собственный pending/stage, выполняет реальное exact-lossless encode, no-replace canonical publish, reopen/seal и commit. При неправильном marker child заканчивает с raw exit **101** до file/DB write. Production не создаёт gate (`#[cfg(test)]`); ни один checkpoint не вызывает abort или kill сам.

F06: file lane ставит marker **после успешной проверки exact bytes на sealed canonical HANDLE**, до отправки запроса на SQLite `BEGIN IMMEDIATE`. F07: SQLite lane ставит marker после pending DELETE и трёх INSERT, **до COMMIT** при ещё открытой transaction. Marker пишется в новый temp, sync и публикуется переименованием в root; после публикации только этот child worker паркуется. Parent ждёт marker с timeout 25 s и проверяет, что Child жив; при любом раннем exit/timeout тест FAIL, RAII прерывает только удерживаемый Child handle. После verified handshake `Child::kill`/bounded reap ≤5 s, raw Windows exit **1** (не in-process error/exit101). На kill/reap timeout FAIL, без fallback к PID/name kill. Bad-marker setup проверен отрицательно для обоих F06/F07; fixture при panic сохраняется вместо потенциального double-panic в parent.

Для каждого F06/F07 после termination: staged v2 manifest/context/pixels точно совпадают по bytes и SHA; final совпадает по bytes/size/SHA; sentinel остаётся. Новый lease на том же root подтверждает pending=1, records/assets/saved=0, v2 ordered warnings, существующий canonical принимается `newly_published=false`. Первый restart commit создаёт ровно один record/asset/saved с исходными UUID, плановым месяцем, warnings, canonical SHA/length и no-replace final; второй restart `lookup_operation` возвращает тот же Saved без дубликатов, stage/final/sentinel неизменны. Это реальное process termination на собственной test child, а не предыдущие in-process F06/F07 failpoints.

## Проверки (Windows x64, pinned offline MSVC, raw exits)

| Команда | Exit | Результат |
|---|---:|---|
| `.pi/t09a-run.cmd test --offline --locked -p sm-storage --lib s02_t09n4 -- --test-threads=1` | 0 | 1 parent test PASS, child test ignored в parent и запускается только с marker; F06/F07 + negative setup |
| `.pi/t09a-run.cmd test --workspace --offline --locked --quiet -- --test-threads=1` | 0 | serialized workspace; storage 117 PASS + 1 intentionally ignored child |
| `.pi/t09a-run.cmd clippy --workspace --all-targets --offline --locked -- -D warnings` | 0 | final source без предупреждений |
| `.pi/t09a-run.cmd fmt -p sm-storage -- --check` | 0 | scoped fmt |
| `python tests/contracts/check_boundaries.py` | 0 | 15 crate boundaries и test-only child/SQL/file checkpoint assertions |
| `powershell.exe -NoProfile -NonInteractive -File scripts/verify-contracts.ps1` | 0 | implemented subset PASS; S01 IN_PROGRESS |

До correction boundary guard имел exit 1 из-за inventory и ожидания прежнего форматирования вызова, а Clippy — exit 101 из-за redundant closures; исправлены и повторены до exit 0. Полный evidence локален под `.pi/`; не публиковать приватные stdout/stderr/пути. Serena language server недоступен (`Active language servers: []`); Codebase Memory и ast-grep использованы для поиска владельца/SQL места, source проверен на диске и runtime тестом.

**Границы:** нет Windows power-loss/NTFS durability доказательства, общего RSS ≤1 GiB, защиты от hostile preexisting writable mappings, unconditional Saved или file+SQL atomicity; нет host/startup/status/clipboard wiring. Прежняя условная модель угроз n3b2 сохраняется. Этот gate закрывает только два узких storage-only process termination cases; F01–F10 целиком и S02 stage остаются открытыми.
