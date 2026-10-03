# S03 — runnable interface seam (не приёмка всего этапа)

Один `shotd`/StorageWorker/ImageServices → shared authenticated client → `shot` и
Tauri/React. Импортированные записи доступны в source-aware pagination без фиктивного
CaptureContext. Preview получает проверенный WebP ≤8 MiB; AVIF/large canonical
использует contained derivative, не WebView AVIF decoder. Canonical read сохраняет
отдельный контракт и больше не аллоцирует весь файл на каждый chunk.

## Пользовательский путь

- GUI: native Drop переносит пути в доступное поле; явное подтверждение профиля
  WebP exact / AVIF SDR90 (lossy RGB), duplicate policy, прогресс/постраничный report,
  pause/cancel/resume/retry. Renderer не обходит FS, не декодирует источники и не
  выполняет SQL/SDK. Batch UUID сохраняется до invoke и между открытиями.
- Library: empty→first Saved auto-preview, новые Saved, страницы после 24,
  generation guards для selection, bounded hex/hash/MIME/EOF и disposal Blob URL.
- Export: PNG/JPEG/WebP/AVIF, max-side, JPEG background, явное назначение или managed
  export, immutable selected asset и stable operation/request при повторе.
- Source metadata: отдельный Owner `ReadSourceMetadata`; TestReadOnly не получает
  import/export/image/private-metadata права. Полная карточка origins и S07 — позже.

Реальная CLI vocabulary (конфиг и пути — явные абсолютные):

```text
shot --config <config> import --path <file-or-folder> --profile avif_sdr90 --duplicates keep_both --batch-id <uuidv4> --wait-ms 60000 --json
shot --config <config> import status --batch-id <uuidv4> --wait-ms 60000 --json
shot --config <config> import report --batch-id <uuidv4> --after-ordinal 0 --json
shot --config <config> import pause|cancel|resume|retry --batch-id <uuidv4> --json
shot --config <config> metadata --record-id <uuidv4> --json
shot --config <config> export --record-id <uuidv4> --operation-id <uuidv4> --format png --output <destination> --max-side 1024 --json
```

Wait0 — admission/status, не обещание Saved. Timeout — raw8 с batch ID/report;
terminal errors/partial — raw7. Export failure сохраняет operation ID. Плановые
aliases, relative-path CLI, JSONL, item-specific retry и полный S07 не заявляются.

## Исполнено

- Focused Rust CLI/GUI/host suites raw0; native session 2/2 raw0, protocol 8/8 raw0.
- Storage default-thread suite после узкой fixture correction: 126 PASS, 1 ignored;
  6 compile-fail doc tests PASS, raw0. Новый test: 26 records/pagination, AVIF preview,
  metadata scope, source deletion, reopen, unchanged canonical.
- Frontend typecheck/lint/build raw0, 16 tests PASS; TS protocol 7/7 и boundaries raw0.
- Actual owned CLI: nested tree 5 inputs → 2 Saved, 1 linked, 2 явных errors (raw7);
  AVIF Saved raw0. Source deletion → PNG marker round-trip/export replay raw0;
  canonical unchanged, protected canonical destination refused. Wait raw8 → same
  batch in reopened CLI raw0; GUI window/reopen не создаёт второй host.
- Final actual Tauri PrintWindow: **1462×1136 physical, DPI144**, три RGB полосы
  imported AVIF видны; private pixel probe нашёл по2592 marker samples каждого цвета.
  Это actual host/list/preview/render, не mock. Cleanup: authenticated stop raw0 →
  exact Test key absence **1168** → own root removed.
- Один full `scripts/verify.ps1`: raw1, fmt/clippy/offline-safety/frontend build PASS;
  workspace tests обнаружили collisions независимых image fixtures с process-wide
  one-worker permit. Production overload/retry не ослаблен: serialized только fixture
  lifetime; default-thread storage rerun raw0. Full suite повторно НЕ запускался,
  оставшиеся fail-fast steps NOT_RUN, результат FAIL сохранён.

Private raw evidence: `.pi/temporary/729743c9-07ce-4cfa-90a1-105d9f5d0a59/`;
final image `.pi/temporary/f8acd881-8a0b-419d-bea4-8f3cb74867a3/owned-window.png`;
full result `.pi/temporary/171bcc02-1f43-4549-b270-058338bae08c/result.json`;
focused correction `.pi/1f7b3165-414b-40c1-b8ea-83c97e41fc22/`.

## Узкое review и исправления

Независимый Sol6.1 review подтвердил runnable путь и не нашёл P0/P1. Два P2
исправлены: `in_flight` суммирует состояния, skipped failures дают
`completed_with_errors` вместо ложного успеха. Existing storage cases — 7 PASS;
независимый public-progress regression — PASS. Настоящий CLI с пустым input
возвращает raw7; guarded cleanup stop0→absence1168→root removed.
Evidence: `.pi/temporary/0738aebb-e745-43c2-9eff-73902245d8f4/`,
`.pi/temporary/c23c71f9-06e1-4686-8959-f7ec723debf6/`,
`.pi/temporary/b7cf604a-b535-4521-99af-f52a9dcacdb4/`.
Повторное узкое review: pass, findings0. Parent проверил actual screenshot выше.
Parent final run `.pi/temporary/6517c014-4659-4ec9-ab4e-fc54bd690d01/`:
workspace tests/Clippy/fmt, image/worker faults/production, storage, host/CLI/GUI и
frontend — raw0. Overall raw1: contract gate обнаружил два устаревших generated
**description**, не изменение wire behavior. Schema/TS comments синхронизированы с
Rust owner; отдельный existing contracts runner — raw0, все 15 шагов PASS:
`.pi/temporary/dd7d5aaa-ec45-4526-b3c9-ee130136522b/result.json`.
Полный suite не повторён; его FAIL сохранён. Итоговая scoped приёмка и остатки —
[result.md](result.md), SHA code slice находится в Git-истории этого отчёта.

**NOT_RUN:** genuine drag/click/SendInput, GUI import/export button interaction,
full S03/S07, signed/clean package, automatic export TTL timer, full-stage performance,
SDK/runtime/FTS5. Native package trust пока fixed trusted host sibling + pinned handle,
не signed resource manifest acceptance. Review/приёмка/commit принадлежат parent.
