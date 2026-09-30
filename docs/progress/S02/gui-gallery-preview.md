# S02 — первая галерея и ограниченный preview (candidate)

На рабочем дереве HEAD `0676315` реализован первый пользовательский срез: authenticated GUI session получает первые 24 committed records, React отображает список и открывает выбранный WebP из host asset ID. Первая страница без pagination/thumbnail; диагностика и capture сохраняются. При `Saved` или `ready` список обновляется, есть ручное обновление. Окно **не запускалось**: `GUI-WINDOW-RUNTIME = NOT_RUN`; видимость реального изображения в WebView и сквозной capture→gallery→preview ещё не подтверждены. Без clipboard/editor/hotkey.

Preview принимает только lowercase canonical UUIDv4, host разрешает идентичность и наличие asset; ни path, ни свободная команда от WebView не принимаются. Legacy hex transfer проверяет размер <=8 MiB *до* конкатенации, а затем offset/next_offset, total_size, mime=image/webp, asset ID, EOF и SHA256 по всем частям. При превышении — `PREVIEW_TOO_LARGE`, повреждении — `PREVIEW_UNAVAILABLE`; React декодирует bounded hex, создаёт `blob:` URL, отзывает при смене/закрытии/unmount. CSP разрешает `blob:` только для `img-src`; boundary gate проверяет это. Отказ preview не затрагивает сохранённый канон. Остаточный риск: существующий host storage read полностью загружает canonical file при **каждом** чанке, прежде чем GUI узнает размер; устранение — отдельная host-owner задача, не проверено runtime.

Проверки в общем рабочем дереве на синтетических fixtures (все raw exits):

| Команда | Результат |
|---|---|
| pinned Node 24.13.0 `pnpm --dir apps/gui typecheck` | exit 0 |
| pinned Node `pnpm --dir apps/gui lint` (boundaries, parity, CSP-only img-src blob assertion) | exit 0 после CSP-изменения |
| pinned Node `pnpm --dir apps/gui test` | exit 0, 14/14 |
| pinned Node `pnpm --dir apps/gui build` | exit 0 |
| pinned Rust 1.98.1 `cargo test --locked --offline -p sm-gui -j 2` | parent exit 0 после итоговых правок, 2 unit + 2 gallery_preview + 10 gui_surface |
| pinned Rust 1.98.1 `cargo clippy -p sm-gui --all-targets --locked --offline -- -D warnings` | parent exit 0 после двух синтаксических правок |
| `scripts/verify-contracts.ps1` | parent exit 0 после GUI `sha2` allowlist; implemented checks PASS, stage_result=IN_PROGRESS |
| pinned `rustfmt --check --edition 2021` на изменённых Rust-файлах | exit 0; parent повторно проверил `model.rs` после правки |
| `git diff --check` | parent exit 0 после итоговых правок |
| PowerShell parser `scripts/preview-gui.ps1` | parent exit 0 после обновления ожидаемых unfinished capabilities и закрытия окна |
| `scripts/preview-gui.ps1 -Action selfcheck` | BLOCKED: build exit 101 (`os error 5`) при замене `preview-target/debug/shotd.exe`, удерживаемого чужим ранее запущенным project-owned host (29.09); не остановлен и не тронут; selfcheck/окно NOT_RUN |
| `cargo fmt --all -- --check` | exit 1: ранее существующий drift вне GUI (application/cli/host/session), не исправлялся |

Первоначально `--locked --offline` дал exit 101 из-за нового прямого `sha2` в crate lock metadata: lock обновлён только `cargo test --offline`, затем `--locked --offline` exit 0. Ранее полный GUI тест дал 9/10: устаревшее ожидание `gallery` в NOT_IMPLEMENTED исправлено, повтор полного GUI 14/14 exit 0. Первый parent contracts gate дал exit 1 из-за пропущенного `sha2` в GUI allowlist, а Clippy — exit 101 на двух синтаксических замечаниях в assembler; узкие исправления внесены, повторные parent gates exit 0. При ошибке обновления React теперь убирает устаревшие записи/preview, а новый выбор сбрасывает прежний spinner; list/preview ожидания ограничены timeout. Независимый scoped static re-review после правок — PASS для кода, не runtime. Скрипт preview-gui теперь не ожидает gallery/library_counts в unfinished и корректно принимает ручное закрытие окна. Selfcheck остановился **до создания нового test root**, потому что сборка не смогла заменить exe существующего ранее project-owned `shotd.exe` из другого тестового root; чужой live процесс не завершали. Отдельный запуск GUI-window на owned test root, затем видимый WebP в нём — NOT_RUN; commit/stage на момент этой записи не создавались.
