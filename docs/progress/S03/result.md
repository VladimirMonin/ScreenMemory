# S03 — принятый рабочий функциональный срез

Статус: **ACCEPT_CODE_SCOPED**, весь S03 и локальный релиз остаются IN_PROGRESS.

## Результат

Файлы/рекурсивные папки → durable accepted snapshot → contained decode/normalize →
проверенный WebP/AVIF → no-replace publish → один SQLite writer/commit → Saved →
source-aware библиотека → действительно видимый Tauri preview → immutable экспорт.
CLI и GUI используют один authenticated host; сохранение не зависит от AI.

- [Codec/worker](image-codecs.md): реальные форматы, orientation/ICC/metadata,
  resize, explicit JPEG background и contained process failures.
- [Inputs/provenance](input-manifest.md), [точные зависимости](dependencies.json).
- [Интерфейсы/runtime](interfaces.md): nested imports, duplicates, source deletion,
  export replay/refusals, reopening, actual physical own-HWND RGB screenshot.
- Migration0005: fresh/empty owned root допускается; populated≤4 production
  **refuses до destructive SQL** без backup authorization. 0001–0004 неизменны.

## Приёмка

Sol6.1 fresh functional review: runnable path подтверждён, P0/P1 нет. Два P2
import-report исправлены; retained independent follow-up PASS/findings0, public
in-flight invariant и actual empty-input CLI raw7 проверены, cleanup1168 выполнен.

Parent integrated run: `.pi/temporary/6517c014-4659-4ec9-ab4e-fc54bd690d01/result.json`.
Rust workspace/Clippy/fmt, image/worker, storage, host/CLI/GUI и frontend — raw0.
Overall **FAIL/raw1** на stale generated descriptions. Исправлены только comments;
existing contracts runner `.pi/temporary/dd7d5aaa-ec45-4526-b3c9-ee130136522b/result.json`
— PASS/raw0. SOURCE-TEXT выполнен отдельно; полный suite не повторялся. Старые
failed runs сохранены. Код и этот отчёт фиксируются одним локальным logical commit;
его SHA — `git log -- docs/progress/S03/result.md`, без self-referential hashes.

## Не заявляется

Genuine drag/click/GUI-buttons/capture/clipboard — NOT_RUN/BLOCKED до own guest.
Populated production upgrade/backup, signed clean package, full performance/memory
matrix, automatic export-TTL timer и item-specific retry — следующие owners/этапы.
Conditional durability/power-loss residuals S02 не закрыты импортом. SDK/FTS5 ещё
не подключены. Следующий полезный milestone — S04 private SDK/runtime и очередь.
