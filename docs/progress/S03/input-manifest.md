# S03 image owner — безопасный вход и handoff

2026-10-02. Foundation scoped `99df8f3`, report `de41adb`, входной checkpoint
`6509b21e0edc24b2e57639d31c9c0720899ea402`. Полная S01/S02 не принята;
[S02 live-stand](../S02/isolated-live-stand.md) остаётся BLOCKED/NOT_RUN.
Этот image milestone не запускает screen/input/clipboard/VM/OS feature operations.
Девять исходных планов не менялись. ADR-011: FTS5-only, SDK вне WebView, AI здесь нет.

## Actual storage, не историческая schema2

| Вход | Фактическое состояние / что обязан сохранить следующий owner |
|---|---|
| Global SQLite schema | SHIPPED migrations0001–0004, applied version4. Не создавать «migration3» поверх существующей0003. |
| Images snapshot | `SchemaSnapshot.images.migration_version=2` — версия images contract, **не** глобальная версия БД. |
| `0002_images.sql` | Реальные `records` содержат capture_context/provenances; `assets.format` разрешает только `webp_lossless`, path заканчивается `.webp`, UNIQUE(record_id,asset_type). Исторические imported_image/avif/immutable triggers из S03 §1.3 не являются текущей schema. |
| `0003_operations.sql` | Capture operation receipts, не import batch schema. |
| `0004_capture_intents.sql` | Pending capture identity и guards. Immutable SQL0001–0004 не переписывать. |
| Capture staging | v1 имеет18полей, v2 —21 с точными warnings/provenance/path policy; это не версия SQL. V1 не recovery-ready. |
| Saved | Только verified canonical + DB commit; WorkerResult/Candidate никогда не Saved. |
| Recovery | Sealed witness/candidate/published/seal и conditional Saved threat model сохранены; hostile prior writable mappings не объявлены закрытыми. |
| Recovery codec | Отдельный существующий1MP cap остаётся. Новые image96MP limits не снимают его и не доказывают global RSS1GiB. |

Storage/host/protocol/UI здесь не изменены. Следующему ingest owner нужна явная новая
миграция после глобальной4 и compatible mapping текущих DTO, не SQL из исторического
плана как готовый adapter. Другого host/asset store/DB не добавлено.

## Runnable API и source map

| Путь / owner | Реальный контракт |
|---|---|
| `crates/codec/src/lib.rs`, `native.rs` | Capture/recovery exact API/fingerprint прежние; новый export-only `encode_lossy90_export`, не canonical tuning. |
| `crates/image/src/lib.rs` | `Options {method,profile,resize,jpeg_background}`; `Operation::{NormalizeCanonical,DeriveExport}`; `Options::canonical` только WebP exact/AVIF90 и full size. Pure `process` — worker/tests. |
| `crates/image/src/inspect.rs` | Magic/container + animation/RAW/HDR/depth/color/page guards до полного decode. |
| `crates/image/src/metadata.rs` | EXIF/XMP/time/GPS/camera observations с bounds и provenance; отсутствующая TZ не берётся из ОС. |
| `crates/image/src/avif.rs` | Static libavif1.0.4/libaom3.11.0; quality90/444/alpha100/speed6/threads2, actual encode/decode/structure/alpha verification. irot до imir; конфликт EXIF — refusal. |
| `crates/image/src/transform.rs` | Common integer fraction + linear-premultiplied Lanczos3; no upscale/no filter при no-resize. |
| `crates/image-worker/src/wire.rs` | Private ABI v1, closed typed method/options, operation ID/hash/length, bounded JSON header + binary body. Hello до input, accepted/encoding phases. |
| `crates/image-worker/src/windows.rs` | Trusted package executable/SHA, one active permit, Job memory/child-count/kill-on-close; kill+OS wait+Job ActiveProcesses0 до return/release. |
| `crates/image-worker/src/main.rs` | Настоящий `shot-image-worker.exe`; stdin/stdout, без arbitrary argv, runtime PATH/credentials/DB/network. |

Пример интеграции без publication authority: создать TrustedWorker **из trusted
package manifest**, затем на image lane вызвать `execute(operation_id, owned_source_bytes,
Options::canonical(profile), cancellation, phase_timeout)`. Input ownership переносится
в child writer без второй host copy. `phase_timeout` ограничивается30s для metadata/decode
и180s для AVIF encode (30s для остальных). В тестах используются только own executable
и checked-in synthetic PNG. WorkerResult отдаёт immutable bytes/report или переносит их
через `into_parts`; host сохраняет retained snapshot при ошибке/cancel и сам выполняет
trusted no-replace publication/DB commit. Не декодировать untrusted source в host через
публичную pure test/worker функцию.

Report: source SHA, normalized-full-input `pixels_sha256`, **отдельный**
`decoded_pixels_sha256`, output SHA/length/MIME/dimensions, normalized_dimensions,
method/profile/color/metadata/applied orientation. Lossy AVIF RGB hashes закономерно
различаются; alpha сравнивается побайтно. Для derivative input hash не переименовывается
в hash уменьшенных pixels. Host обязан сверить operation/method/profile/hash/length и
текущий attempt перед trusted публикацией; содержимое report не обычный log payload.

## Форматы и metadata matrix текущего seam

- PNG/JPEG/WebP: static input; PNG/JPEG EXIF/XMP и WebP EXIF/XMP packets;
  RGB/gray ICC через LCMS2, WebP RGB ICC поддержан. Compressed PNG XMP и extended
  JPEG XMP не обещаются: unsupported packet variant явно отказывает/не заявляется
  как извлечённый. Metadata summary — bounded camera/time/GPS subset, не все MakerNotes.
- AVIF: static primary + alpha не считаются temporal frames; EXIF/XMP/ICC,
  SDR8 sRGB/untagged allowed path, irot/imir. Crop/non-square aspect и unsupported
  transfer/primaries/depth/gain-map требуют conversion. Не silent first-frame.
- BMP/still GIF: pixels; EXIF/XMP extraction не объявляется реализованным для этих
  контейнеров. Animated GIF отвергается.
- Classic single-page TIFF: gray/RGB/RGBA8, uncompressed/LZW/Deflate/PackBits.
  TIFF EXIF bounded IFD path; metadata слишком далеко в файле для8MiB extent явно
  отказывает. XMP tag700 summary здесь не заявлен. BigTIFF/JPEG-compressed TIFF/RAW/
  multipage не выдаются за supported минимум.
- Output: PNG/JPEG90-444/WebP exact/WebP export90/AVIF90-444. Source/private tags
  не передаются encoder; прозрачный JPEG требует explicit background и linear-light
  composite. Canonical source bytes никогда не изменяются.

## Зависимости и границы будущей приёмки

[dependencies.json](dependencies.json) — exact MSVC target closure, archive SHA,
licenses/repositories/build-script SHA, Cargo.lock SHA; официальный source bootstrap
записан отдельно, последующие gates offline+locked. CODEC инструкция исправляет stale
0.37.0 на фактический libwebp-sys0.14.4. VS CMake/Git Perl нужны только build-time;
worker выполняется с пустым env/PATH. Packaging owner обязан включить executable,
manifest/hash и notices всех native/Rust dependencies; clean-package/install gate
здесь NOT_RUN, отсутствие зависимостей от внешних image utilities не подменяет его.

Не закрыты этим owner: import migrations/journal/source snapshots/attempt policy,
protected export destinations/registry/TTL, common RPC/CLI/GUI/native drop, shared
capture+background memory admission, full96MP acceptance/RSS и500-cycle stress,
full S03 и S02 live acceptance. Next owner не получает право ослабить Saved/recovery
guards или заменить canonical thumbnail. Scoped evidence — [image-codecs.md](image-codecs.md).
