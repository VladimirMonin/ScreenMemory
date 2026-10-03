---
applyTo: "crates/runtime/**,packages/lmstudio-bridge/**,crates/host/src/processing*.rs,docs/progress/S04/**"
name: "RUNTIME.Sdk"
description: "Читай при изменении private SDK/supervisor, host execution/ModelManager, load-intents/instance leases, uncertainty, секретов и offline package staging."
---

# RUNTIME — host-only published SDK seam

`sm-runtime` и `packages/lmstudio-bridge` — отдельный native/private adapter, не
WebView и не application RPC. Host/queue решают policy/authorization/revisions;
SDK не владеет SQL, каноном, категориями или поиском. ADR-011: только vision,
нет bridge embedding/download методов. SDK dependency closure имеет **свой** exact
`pnpm-lock.yaml`, вне frontend workspace; install только frozen/ignore-scripts после
source/license/lifecycle preflight. Actual SDK Apache LICENSE сохраняется несмотря
на metadata MIT; provenance/notices — в package, не исправлять upstream bytes.

`BridgePackage::open` принимает trusted absolute install directory, pins shipped
runtime/modules по manifest, без wire/ENV/PATH/CWD resolution. Staging script
копирует уже pinned Node24.13.0 и runtime closure/actual licenses, не устанавливает
или скачивает tools/models. Manifest — trusted-directory integrity, не signed
release/sandbox против hostile same-user. `private_node_command` очищает ENV и на
Windows добавляет **только** SystemRoot из bounded `GetWindowsDirectoryW`.
Не наследовать SystemRoot/NODE_OPTIONS/NODE_PATH/PATH/LM_API_TOKEN от caller.
Node запускается с `--no-global-search-paths --no-addons`: optional ws native
plugins/global user modules не authority и не участвуют в shipped closure.

Supervisor держит единственный owned child/Job (active process1, commit512MiB,
kill-on-close), max3 explicit starts, без auto replay. Private stdin/stdout:
LE32 JSON≤4MiB, отдельный PNG payload≤20MiB, serialized dispatch, bounded read pump;
bridge active≤8, mutation1, terminal status≤64, uncertain lane fenced. Initialization
содержит private random authentication cookie, generation и secret authority token;
они не argv/ENV/log. Endpoint — явный `ws://127.0.0.1:<numeric-port>`, без guessing,
remote LM Link и cascading; load `deviceIdentifier:null`.

`LoadIntentStore::commit_before_dispatch` обязан реально commit exact plan до SDK;
`record_outcome` сохраняет identity/config/uncertainty. Durable queue реализует этот
порт через существующий SQLite owner, не новый writer. Specific load handle и
immutable instanceReference принадлежат только текущей generation. Inventory не
adopts чужие/пережившие restart instances; load collision/mismatch не retry. Queue
persist runtime-call intent/attempt/epoch и publication fence остаются её обязанностью.

Host `processing_execution` владеет одной serialized execution lane/ModelManager:
foreground приоритетен, после четырёх foreground доступен queued background consumer.
`SqlLoadIntents` реализует SDK port через прежний SQLite worker; ни модель, ни SDK
не работают под SQL/worker lock. Совместимые exact load parameters переиспользуют
current-generation owned instance, а каждый consumer получает свои frozen sampler
parameters. Incompatible reload/foreign-or-unknown instance budget отказаны без eviction.
Inventory/readonly Reconcile не mint/adopt OwnedInstance. Background Allow и query
Allow/Ask/Forbid независимы; Ask approval только trusted host configuration, не wire.
Default Off не стартует private SDK. Production composition/runtime config и exact
backend credential authority — у [HOST.composition](HOST.composition.instructions.md)/
[SEC.credentials](SEC.credentials.instructions.md). Explicit Owner Inventory проходит
bounded8 actor lane вне SQL, может стартовать bridge при Off, но не модель; queued
inventory с потерянным waiter не dispatch, busy/running lane даёт bounded refusal.
Read settings/status/overview/doctor не имеют SDK side effects. Native61s isolated reactor gap проверяет реальные
lease expiry/late publication fences, но не является actual OS sleep proof.
SQLite fences/outcomes/retained derivative bindings и recovery описывает
[DATA.storage_owner](DATA.storage_owner.instructions.md); bootstrap/scopes —
[HOST.composition](HOST.composition.instructions.md)/[APP.session](APP.session.instructions.md).

`PreparedInferenceImage::prepare` — единственный native constructor: specific saved
asset через S03 PNG export1024/2048, StripPrivate, checksum и retained ExportReadLease.
Нет user paths, external source reread, EXIF/GPS или arbitrary bytes constructor.
После uncertain inference **caller удерживает derivative/lease** до durable
reconciliation; убитый Node не доказывает остановку LM Studio. Принятый канон не кеш.

Cancellation ACK не settlement. Native ждёт final outcome+ACK, при grace3s/IO/caller
future drop fences дальнейшую mutation; partial read принадлежит persistent pump.
Shutdown/wait/Job-empty закрывают только собственный child. Prediction max1…4000
общий; незавершённый/невалидный JSON не result. Off и Budget100 fail-closed unsupported
до behavior-verified runtime/model capability; BackendDefault — явный выбор, не Off.
Reasoning text не логируется, missing reasoning usage — null, не0.

Published SDK2.0.0 unload identifier-only, без atomic instance comparison:
`unload_owned` **unsupported** для manual/host Idle/AfterBatch/reload cleanup/Off/
shutdown/recovery, не precheck→unsafe unload, hidden REST или UUID-argument fallback.
Private alias — collision hygiene, не reservation/authority. Lease release, own Node
stop и asyncDispose не physical eviction. Native positive TTL передаётся explicit
specific load intent, но `LoadObservation.native_ttl` возвращает requested_seconds
и verification=not_verified, отдельно от load readback. Это не host lifecycle/AfterBatch
PASS или safe unload. Requested unsupported lifecycle нельзя silently ignore/switch:
queue/UI owner блокирует такой mode до dispatch. Strong parent decision не stage acceptance.

Проверки: affected `cargo test/clippy -p sm-runtime --locked --offline`, build
production `shot-image-worker` до composed tests; pinned Node + existing protocol
TypeScript compiler на bridge tsconfig; Node `--test test/sdk.test.mjs`. Native test
создаёт UUID roots/own synthetic WebSocket server, S03 канон и packaged clone; secret
canaries и env-poison живут только в owned test child. Это actual published-module
protocol/packaging proof, **не real LM Studio/model или signed clean install**.
Current execution closure проверяется отдельным serial affected invocation:
`cargo test --offline --locked -p sm-host --features processing-test-seams --test
processing_execution -- --test-threads=1`; existing fixture staged clone/SDK owner
используются без повторного standalone SDK/bootstrap suite. После test feature
обязательны default production build/check и affected clippy в обоих feature paths.
Только own UUID roots/Child handles; failure PID не authority для общего name-based
cleanup. kill_on_drop/Job close не подтверждают backend settlement; historical unknown
fixtures/PIDs сохраняют NOT_RUN/BLOCKED, не задерживая независимые gates.
