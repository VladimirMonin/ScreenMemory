---
applyTo: "crates/host/**,crates/cli/**,scripts/preview.ps1,scripts/preview-common.ps1,scripts/smoke-host-cli.ps1,scripts/test-preview-safety.ps1,scripts/test-foundation-smoke.ps1,scripts/test-preview-rejection.ps1,scripts/README.md,docs/progress/S01/t05d-host-cli.*,docs/adr/009-host-cli-composition.md"
name: "HOST.Composition"
description: "Читай при изменении реальных shotd/shot бинарей, bootstrap-порядка, trusted preview configuration, CLI-вывода/exit codes, autostart/owned child и shutdown-порядка."
---

# HOST/CLI — composition root, тонкий клиент и developer preview

Источники: S01 §§4, 6.1–6.5, 10.2–10.3, 11.5; [ADR-009](../docs/adr/009-host-cli-composition.md);
принятые [ADR-003](../docs/adr/003-root-isolation.md), [ADR-004](../docs/adr/004-storage-worker.md),
[ADR-005](../docs/adr/005-local-credentials.md), [ADR-007](../docs/adr/007-native-pipe.md),
[ADR-008](../docs/adr/008-session-client.md). `sm-host` (bin `shotd`) — единственный
composition root: explicit absolute root, `HostLease` до SQLite, один SQLite writer,
credential policy, bounded logger, sealed listener, `host.ready` только после успешного
bind; AI off и bridge не стартует. S03 optional ImageServices использует тот же StorageWorker
и SessionHost::with_images. Fixed `shot-image-worker.exe` берётся только рядом с
trusted host executable, проверяется/pins до lease/DB/native owners; нет wire/PATH/
ENV worker path. Отсутствие sibling не рекламирует image methods; package trust
пока основан на trusted native directory, не release/signed manifest acceptance. `sm-cli` (bin `shot`) — только клиент принятого
`sm-session::ApplicationClient`: не открывает SQLite, не линкует storage owner, не
выбирает второй host.

Bootstrap-порядок, exit codes (2/4/6/7/8) и shutdown (receipt=stopping; stop admission →
bounded drain уже принятых соединений → listener → private execution drain → storage close с освобождением lease →
bounded log flush в бюджете 5s) не упрощать: `lease` живёт, пока SQLite жив; ложный
«все handles закрыты» запрещён. Обычный выход CLI host не останавливает.

Production `shotd` вызывает `bootstrap_from_installation`: после root/lease/DB identity,
до logger/capture/listener читает fixed `config/processing-runtime.json` (≤4096 bytes,
strict schema1, exact library UUID, numeric port1…65535, authentication=`credential|guest`,
ask_load_approved, image_side1024|2048, model_autostart=false). Отсутствие файла — runtime
unconfigured/Off. Invalid/autostart=true/identity mismatch/unknown fields дают отказ,
не fallback. Package — только fixed `sdk-runtime` рядом с trusted native image;
worker — прежний sibling, package bytes pins до запуска SDK. Config не содержит package
path/endpoint string/token/model key; ENV/PATH/CWD и application load booleans не authority.
Изменять этот host-owned файл только при остановленном собственном host; configure не
делает inventory/load. Exact backend vault key и bounded stdin — [SEC.credentials](SEC.credentials.instructions.md).

S04 `bootstrap_with_processing` принимает только native `TrustedProcessingRuntime`:
trusted absolute package, explicit numeric loopback endpoint, SecretAuthority и pinned
TrustedWorker. Это не application DTO/ENV/PATH configuration, не implicit load consent.
Recover writer ledger до listener, actor независим от session waiter. Default bootstrap
без этой authority остаётся Off/runtime-unconfigured и не запускает модель. Technical
result не S05 description; private ModelManager/fences — [RUNTIME.sdk](RUNTIME.sdk.instructions.md).
При shutdown сначала bounded cancel/drain execution; timeout abort только own task/child,
не ложный clean exit/backend settlement. Uncertain SQL holds переживают drop local handle.

Порядок внутри шага 5 фиксирован: сначала explicit owner enrollment (и reopen) под удержанным
`HostLease`, и только после успешного enrollment — bounded logger и minimal services; listener
остаётся последним. Отказ enrollment поэтому не создаёт file logger: отчёт уходит bounded typed
событием в stderr с фиксированным policy exit code, а кэшевый log sink существует лишь при
успешном enrollment. Композиционный тест держит этот порядок; logger нельзя «передвинуть»
раньше credential policy без change note/ADR.

Client-конфигурация — только явный absolute `--config` JSON (`schema_version`,
`environment` = `test`, absolute `root`, canonical `library_id`, absolute
`host_executable`); никакого CWD/ENV/PATH fallback, никаких product/dev namespace в этом
срезе. `library_id` берётся из реального `shotd --init` отчёта; enrollment и очистка —
только точный synthetic `ScreenMemory.Test.V1/library/<id>/owner`, без enumeration и
чужих targets. Autostart — максимум один запуск fixed absolute image, bounded
retry/discovery 5s, без retry на credential/identity ошибке, и завершение только
собственного недостигнутого child, никогда чужого PID/имени образа. Host переживает CLI,
поэтому CLI снимает inherit у своих standard handles до spawn и пишет stderr хоста в
`<root>/cache/host-stderr.log` (truncate на запуск), а не в пайп родителя.

`--json` — ровно один JSON в stdout, диагностика в stderr; обязательный провал doctor-проверки
даёт nonzero, отсутствие optional AI/runtime — нет; незавершённые capability честно
`UNSUPPORTED`/`NOT_RUN`, никогда `PASSED`. Проверки: `scripts/verify.ps1`
(`HOST-COMPOSITION`, `CLI-SURFACE`), `scripts/preview.ps1` (developer preview, одна
команда) и отдельный `scripts/smoke-host-cli.ps1` (явный synthetic executable smoke вне
обычного verify, immutable `.pi` evidence, точная очистка Test credential). Отдельные
`.pi/temporary`/`.pi/preview` roots — синтетические, без пользовательских данных, моделей
и сети. Whole S01 остаётся `IN_PROGRESS`: существующие host/CLI/GUI и WebP не заменяют
SDK/FTS5/package proofs S04/S06/S08; embeddings/vec исключены ADR-011.
Preview и verify используют `.pi/staging-target-t09b`, не общий `.pi/preview-target`,
который может удерживать чужой host. MSVC failure немедленно возвращает raw exit;
Cargo всегда offline. `smoke-host-cli.ps1` проверяет текущую storage schema 8 (не
JSON config schema 1). Error cleanup использует тот же guarded stop→absence→remove,
а не удаляет root/credential после неподтверждённого stop.
`scripts/test-foundation-smoke.ps1` — отдельный composed synthetic host/CLI/GUI
regression; его запуск не превращает весь S01 в ACCEPTED.

S04 `shot --config <absolute-config> processing status|jobs|inventory|policy|profile|save-profile|self-test|background-self-test|analyze|batch|pause|resume|cancel-batch|cancel|retry|execution|reconcile` всегда выдаёт один JSON; vocabulary/flags — [interfaces](../docs/progress/S04/interfaces.md).
Policy/Profile mutations используют typed revisions и stable operation ID; self-test требует
явный operation/policy revision/record/profile ID. `gui_processing` — тот же shared client,
не второй runtime. Inventory — отдельная Owner action: допускает bridge start при Off,
но никогда model load/adoption. Read status/doctor/overview не запускает SDK.

S05 `shot knowledge read|edit|categories|category` и `gui_knowledge` используют один
StorageWorker/KnowledgeApi и ApplicationClient; JSON vocabulary, bounded refusals,
revisions/replay/sensitivity — [DATA.knowledge](DATA.knowledge.instructions.md).
Это manual-only карточка при Off, не Analyze/OCR publisher. Config schema1 unchanged.
S06 `search.request`/`gui_search`/`shot search` — тот же StorageWorker/SearchApi;
лексический запрос не запускает SDK. Контракт — [DATA.search](DATA.search.instructions.md).
CLI dispatch выбирает первый top-level command token, не зарезервированное слово
в любом argv: `knowledge edit --title search` остаётся ручной правкой. Значения flags
не меняются; выбранный parser по-прежнему отказывает unknown/missing/duplicate flags.
Regression — `knowledge_args`/`search_args`, strict host schema oracle — `cli_surface`.

S03 `shot --config <absolute-config> import --path <absolute-file-or-folder>`:
`--profile webp_exact|avif_sdr90`, `--duplicates reuse_record|keep_both`, optional
`--batch-id <uuidv4>`, `--wait-ms 0..180000`. `import status/report/pause/cancel/resume/retry`
принимают `--batch-id`; report — `--after-ordinal`. `metadata --record-id <uuidv4>`;
`export --record-id <uuidv4> --format png|jpeg|webp|avif`, optional `--operation-id`,
`--output <absolute-path>`, `--max-side 320|1000|1024|2048`, `--jpeg-background R,G,B`.
Image commands всегда выдают один JSON. Wait0 означает admission/status, не Saved;
wait timeout — raw8 с batch ID/report, incomplete terminal/partial — raw7.
Export error сохраняет operation ID; повтор требует того же request, не нового UUID.
Это реализованный seam vocabulary, не все плановые aliases/flags S03/S07.

Owned preview cleanup — только собственный root и только после проверок: канонический путь
строго `<repo>/.pi/preview/<canonical-uuidv4>`, ownership-marker с тем же UUID (создаётся
только preview create), **независимый init-issued binding**
`.screenmemory-preview-library.json` `{directory_uuid, library_id}` и совпадение
binding/config/root/schema/test/trusted image через `Resolve-ValidatedPreview`.
Binding — create-only из успешного собственного `shotd --init` нового root; не из
`preview.json`, не adoption legacy. Отсутствующий/неверный binding сохраняет данные
и credentials. Успешный authenticated selfcheck связывает library/image, но не
filesystem root, поэтому не заменяет эту проверку. Иначе nonzero без side effects.
Validated tuple и hashes config/binding замораживаются; перед stop, credential delete
и root delete перепроверяются. При mutation — REFUSED, возможный host локально BLOCKED,
root/credential сохраняются. Это helper ownership guard, не sandbox против согласованной
подделки всех файлов/конкурентного hostile same-user TOCTOU. Перед удалением: явный stop с проверкой exit,
bounded `Wait-OwnedShotdGone` и проверка отсутствия **своего** `shotd.exe` (никогда чужой PID
или имя образа); удаление точного synthetic Test credential — через прямой вызов `cmdkey.exe`
с argument list, никогда через shell, и с подтверждённой absence; и только затем — удаление
root с повторной проверкой marker/UUID. Любой неуспех/неопределённость сохраняет root и
credential и даёт non-zero exit. Общий `.pi/temporary/preview-build.log` больше не пишется:
каждая сборка/команда получает новый UUID output root (immutable evidence). Негативные
проверки — `scripts/test-preview-safety.ps1` (вызывается из `scripts/test-verify.ps1`, т.е.
mandatory), только синтетические roots/процессы. `scripts/test-preview-rejection.ps1`
дополняет composed regression: реальные initialized A/B и живой B, подмены config/binding,
прямой CLI cleanup и mutation после GUI arm; никакого desktop capture/clipboard/model calls.
