---
applyTo: "crates/image/**,crates/image-worker/**,crates/storage/src/image_services.rs,crates/storage/src/import_db.rs,crates/isolation/src/image_files.rs,scripts/image-codecs.cmake,.cargo/config.toml,tests/fixtures/images/**,docs/progress/S03/**"
name: "IMAGE.Pipeline"
description: "Читай при изменении S03 image engine, ingest/export services, immutable source snapshots, metadata/color/resize и private worker/package boundary."
---

# IMAGE — private runnable image seam

`sm-image` — in-memory engine, без FS/SQL/сети/моделей. Недоверенные bitstreams и
AVIF исполняются через `sm-image-worker::TrustedWorker::execute`, на image lane,
не SQL/UI/event thread. `process` открыт для worker и synthetic tests, не для
прямого host import decode. Capture/recovery exact API не меняется; отдельный
recovery cap1MP не снят.

Вход выбирается по magic/container: PNG/JPEG/WebP/AVIF/BMP/still GIF/classic single-page
TIFF. Animation, RAW/CFA/DNG, HEIC, HDR/high-depth без conversion policy отвергаются,
не превращаются в first frame или SDR. Tagged RGB/gray ICC преобразует static LCMS2,
relative-colorimetric + black-point compensation; unknown/unsupported color отказывает.
EXIF orientation1–8 физически применяется один раз. AVIF irot→imir имеет приоритет;
EXIF conflict возвращает OrientationConflict, crop/non-square aspect — explicit refusal.

Metadata: EXIF kamadak, namespace-aware quick-xml; 8MiB packets, ICC≤4MiB,
2000tags, XMP depth64/events100000, summary text4096. DTD/entities не раскрываются,
XML/URL/внешние документы не читаются. Time с absent offset остаётся без UTC,
EXIF/XMP taken наблюдения сохраняются отдельно. GPS только валидной парой, не geocoding.
Reports содержат source observations, не попадают в обычные логи.

API: `Options {method,profile,resize,jpeg_background}` с закрытым
`Operation::{NormalizeCanonical,DeriveExport}`. `Options::canonical(profile)` допускает
только WebP exact/AVIF90; canonical с resize отвергается. Export задаётся отдельно
через DeriveExport; camera/GPS source доступны в `metadata::MetadataSource`.

Canonical profiles: прежний WebP exact и AVIF SDR8 quality90/444/lossless alpha,
speed6/threads2, full-range BT709 matrix, BT709 primaries/sRGB transfer. AVIF RGB
lossy, невидимый RGB при alpha0 не обещается. `pixels_sha256` — normalized full
input; отдельный decoded digest — реальный результат codec verification, не тот же
hash под другим именем. Derivative report также содержит normalized_dimensions.
PNG/JPEG/WebP/AVIF exports заново кодируются без source/private tags; JPEG transparency
требует явного RGB background. WebP quality90 export отделён от exact canonical.
No-resize не фильтрует; resize — integer common fraction/no upscale и
linear-premultiplied Lanczos3 для max-side320/1000/1024/2048 и width/height.

Worker: trusted manifest absolute executable + SHA256, pinned write-denying handle,
без PATH/shell/runtime env/secrets; ABI hello до input, operation/hash/length framing.
Один active permit, input≤512MiB/output≤128MiB/header≤256KiB; controlled buffers≤768MiB
(оценка может отвергнуть допустимые96MP без hidden resize), Job Object process commit
1280MiB/active-process1/kill-on-close. Decode/metadata≤30s, AVIF encode≤180s.
Cancel/timeout kill только собственного Child; return/release лишь после OS process
wait + Job ActiveProcesses0. TerminationUnproven оставляет permit poisoned.
Job accounting может обновиться позже process signal — отдельный bounded poll.
Это containment, **не sandbox от same-user exploit** и не утверждение global RSS1GiB.

WorkerResult/Candidate — bytes/report, не Saved/DB witness/publication capability.
Native coordinator проверяет operation/method/profile/hash/length и сам публикует
immutable canonical через существующий storage owner; worker не получает destination/DB
authority. `execute(..., input: Vec<u8>, ..., deadline: Duration)` потребляет только
рабочие bytes: coordinator **до dispatch** сохраняет accepted immutable source snapshot,
не удаляет его при worker error/cancel и владеет retry policy. Worker не имеет FS cleanup
authority. Deadline — per-phase cap, не обещание суммарного wall-clock deadline.
No auto retry/crash loop.

Source bootstrap — отдельный разрешённый шаг, не runner. Версии/SHA/licenses/build
review — `docs/progress/S03/dependencies.json`; Cargo.lock точен. Static libavif1.0.4 /
libaom3.11.0 generic C, LCMS2; build-time VS CMake/Git Perl уже установлены, global
setup/download отсутствует. `.cargo/config.toml` фиксирует offline CMake source
policy; native external override env запрещён runner. Runtime этих tools не требует.

Verify явные steps: TESTS-IMAGE, TESTS-IMAGE-WORKER, IMAGE-WORKER-FAULTS и затем
IMAGE-WORKER-PRODUCTION. Feature worker-test-seams — только fault test build, **не**
shipped binary. Package manifest/clean staging, UI и full S03 acceptance не выдаются
за готовые из codec или service tests.

## Ingest/export service API

`sm-storage::image_services::ImageServices::new(StorageWorker, Arc<TrustedWorker>)`
реализует `sm-application::images::ImageLibraryApi`. Он использует существующие
единственные SQLite writer и file lane; между import items уступает lane уже
принятым capture/read requests. Native composition передаёт trusted package worker
и один общий service в `SessionHost::with_images(Arc<dyn ImageLibraryApi>)`.
Без подключения optional port методы честно возвращают MethodNotSupported.

Owner scopes: ImportFiles, ExportImages, отдельно ExportSourceSidecar. RPC:
`library.import`, `.import.status`, `.import.report`, `.import.control`, `library.export`;
shared Rust Client имеет typed методы, TS package экспортирует generated image DTO.
Import reports дают record/original/current asset IDs для реального preview; legacy
capture-only `page_records` не подменяет imported records фиктивным CaptureContext.
`StorageWorker::library_page` — source-aware listing без фиктивного capture context;
интеграция host/client/CLI/GUI использует эти же services. `preview_asset` через
original file lane создаёт bounded WebP derivative, отдельный от canonical read;
`record_metadata` требует ReadSourceMetadata, reports не попадают в обычные логи.

До worker dispatch source копируется в `data/staging/imports/<snapshot>/source.bin`;
`manifest.json` публикуется последним, связывает library/principal/request/IDs/profile,
identity observations, длину и SHA256. Это сохранный input, не cache: cancel, worker
failure и reopen его не удаляют; retry не обращается к удалённому исходному файлу.
Новый canonical независимо декодируется вторым contained worker, затем no-replace
publication + удерживаемый seal + короткий SQLite commit, только потом Saved.
ReuseRecord связывает новый origin с проверенным imported original; KeepBoth создаёт
отдельные IDs. Discovery: 128 roots/depth, budget100000 entries, explicit Resume
увеличивает budget до10000000; limit/abandoned видны в report, молчаливого полного
завершения нет. Cancel прекращает discovery; последующий Resume возвращает только
удержанный input, не усыновляет отменённые unaccepted files.

Schema5 не меняет SQL/checksums0001–0004. Fresh/valid empty owned schema≤4 допускаются;
пополненная schema≤4 без verified backup отказывает read-only preflight до migration.
Production backup-authorized upgrade не предоставлен; lossless rebuild проверяется
только на собственном stopped synthetic fixture с проверенной SQLite копией.

Export фиксирует Current/explicit asset и metadata snapshot один раз. Durable intent
предшествует no-replace publication; replay проверяет pending output по checksum, не
усыновляет изменённый/чужой файл. Полный saved replay возвращает existence=not_checked,
не обещание наличия файла сейчас. Receipt содержит реальные dimensions/digest/profile
и applied parameters. Исторические source destinations, protected roots/reparse и
canonical aliases отвергаются; поддержаны explicit local NTFS paths.
`destination_path=null` использует `cache/export` с24h TTL. Source sidecar только при
явном policy+scope, partial group/retry обозначены явно. Native `begin_export_read`
проверяет файл и удерживает read lease; `cleanup_expired_exports` удаляет только
зарегистрированные просроченные members по verified handles, не leased group,
не неизвестные cache files и никогда не source/canonical. Автоматический timer и
выдача preview URL принадлежат native composition, не WebView.
