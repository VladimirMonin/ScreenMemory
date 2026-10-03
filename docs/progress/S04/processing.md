# S04 — durable execution / checked bounded handoff

Parent: **ACCEPT_CODE_SCOPED**, [итог/evidence](result.md); не whole-S04 acceptance.
Ниже сохранён final-tail writer handoff на base `c0648dca`, без writer staging/commit.
SDK production sources/accepted bootstrap не переписывались;
прежний PARTIAL checkpoint и все FAIL receipts сохранены.

## Реальные owners

- Sole SQLite owner — прежний sm-storage worker; additive0006 принято только scoped, immutable0001–0005 сохранены; production upgrade gate не закрыт.
- Host/private scheduler: bounded32 admission, transactional claim/stable attempts, 60s lease/15s heartbeat, 45s suspend-gap fence, foreground priority с background после четырёх foreground consumers.
- Frozen record/original asset ID+SHA, profile revision/fingerprint, authorization epoch; private non-deserializable ExecutionFence, нет claim RPC. SQLite intents committed до SDK load/inference dispatch; SDK вне SQL/worker locks.
- Host ModelManager serial/singleflight, current-generation owned instances, durable consumer leases. Совместимые load parameters переиспользуются с собственными frozen sampler parameters; incompatible reload и foreign/unknown memory refuse, без adoption/eviction.
- `bootstrap_with_processing` принимает explicit trusted absolute package/numeric loopback/SecretAuthority/TrustedWorker configuration, не wire flags. Default bootstrap/Off/model.autostart=false не запускают SDK. Host status отражает наблюдавшийся private bridge state; doctor не грузит модель.
- Distinct TechnicalOwnerSelfTest/TechnicalOwnerBackgroundSelfTest реально идут через host→shared authenticated session/client→staged published SDK→own synthetic WS→committed canonical/S03 prepared PNG+ExportReadLease. Только technical metadata, не S05 description/knowledge. Analyze остаётся blocked:handler_unavailable.
- Off/profile/lease fences запрещают late publication. Pause admission не отменяет уже принятого consumer. Session waiter/drop не владеет actor task.
- Backend uncertainty сохраняется в attempts/calls/consumer leases, глобально fencing lane. Durable operation/asset/principal holds защищают derivative от TTL cleanup; reopen возвращает настоящий read lease даже после expiry. ACK/Node kill/late response не settlement и не replay. Reconcile read-only; физически недоказанная uncertainty не снимается.
- Known settled SDK response + failed publication transaction отдельно становятся Interrupted/PUBLICATION_NOT_COMMITTED; нет ложного Succeeded или выдуманной GPU uncertainty. SDK не повторяется автоматически.
- Отдельные background/query load permissions и host-only Ask approval. только NoHostEviction/BackendDefault; прочие retention/reasoning modes unsupported. Token cap4000, temperature0…1, independent K/V cache mapping.

## Проверено — raw evidence

Все пути ниже относительны `.pi/temporary/`; native exits из result.json, не generic stage text harness.

| UUID | Outcome |
|---|---|
| `575d4c93-1f97-4b6e-aee2-3873cee4fcc3` | affected sm-host/sm-storage offline locked check raw0 |
| `c35fad35-d733-4b40-a11c-2057e0b5df92` | public SDK execution test raw0: bounded32, load1, priority/fairness, pause, invalid/truncated output, profile race, Off uncertainty/reopen/late denial |
| `cb8c342b-5196-4881-803b-18e6f6336d5d` | real publication SQLite constraint rollback/reopen raw0 (до улучшения explicit Interrupted) |
| `2118ac84-effc-4c14-90ee-7698e250314e` | actual load intent rollback/load0, explicit retry, Ask/background permissions, readback mismatch, load-pending Off/reopen raw0 |
| `08547db6-f948-48a8-b62b-a82af8b09b84` | actual S03 Saved canonical+asset+origin v5 additive rollback/upgrade/reopen raw0; public SDK + expired derivative re-pinning raw0 |
| `57d432d1-9d88-4339-bbe7-05087e4d9a03` | real61s isolated reactor suspend/expired SQLite lease test PASS; overall command raw101 из-за sibling parallel BRIDGE_ALREADY_OWNED. Harness SDK-owner mutex исправлен. Это НЕ PASS всей команды и не fresh final-byte sleep proof |
| `7aa16641-a436-494a-b59d-9b0696d29a6e` | exact latest known-publication Interrupted и load-failure matrices PASS; overall raw101: main guessed maxTokens scalar, SDK actually returned checked/value object. Assertion corrected to require checked=true/value4000 |
| `75ff6386-f4c5-41f8-a149-d0256d27bf6f` | final public execution raw0, включая sampler reuse/actual SDK params/expired read-lease reopen; schema generator raw1 AlreadyExists (`--output schemas`), TS/clippy subsequent steps NOT_RUN |
| `44e1001f-b02b-485b-a416-2da1118f4af7` | scoped pinned rustfmt raw0; subsequent compile raw101 because BridgeState lacks PartialEq; assertions corrected to exact matches! |

Actual Saved v5 fixture: fresh UUID root, real S03 public import/verified canonical/origin; after owners close only unaccepted v6 tables/receipt removed to restore v5. Then readonly preflight, real migration rollback, upgrade/reopen, canonical bytes/origin invariance and foreign_key_check. Не live-user upgrade и не вручную придуманная asset/origin строка.

Historical failures не удалены: new runs `f3f56053…`, `3cddfe38…` (test API compile), `2856cf0c…` (fixture stdio), `19ce2084…`, `6efcff06…`, `de9a11eb…` (synthetic source path normalization), `03261684…` (mutable lease compile). Они сохранены в ignored raw roots. Ранние control-only evidence: `4d25104a…`, `b9b9b67a…`, `03018200…`, `2718b7c5…`, `e1b9a8e6…`, `57a039a1…`, `5c3b5d12…`; прежние FAIL `e0ec5164…`, `3448c142…`, `30f9030b…`, `0c527833…`, `a1330c14…` остаются историческими, не переинтерпретируются.

Serena Rust LS unavailable (`Active language servers: []`); Codebase Memory owners/trace и ast-grep load_owned дали реальные source facts. Runtime доказан тестами, не графом.

## Final-tail evidence — отдельное от historical FAIL

Pinned Rust1.98.1 через existing InvokeVerificationProcess→cmd `.pi/t09b-run.cmd`,
offline+locked; Node24.13.0. Новых generator/wrapper/harness layers нет.
Existing harness пишет generic S01/IN_PROGRESS — принимать только ниже указанные
step/native_exit, не выдавать generic текст за whole-stage verdict.

| UUID | Actual commands / native outcomes |
|---|---|
| `94911dd7-cf0f-419b-926a-a8496aa895ed` | `cargo run --offline --locked -p sm-protocol --example generate_schemas -- --output <fresh absent UUID child>/rust-generated` raw0; `node packages/protocol/scripts/generate.mjs --output <fresh absent child>/ts-generated` raw0; опубликованы четыре schemas/TS. Subsequent clippy raw101: large Response variant |
| `d6b0c62d-ecea-4385-b20d-f44f05d6e3cb` | clippy raw101: large native storage Reply variant; узко исправлены Box-indirection в application Processing reply/native Settled, без wire changes |
| `30ffc760-82d7-4524-9f8a-0a6d3a34cfe2` | affected default и processing-test-seams all-target clippy `-- -D warnings` raw0 |
| `8c7276a7-d7d7-4c04-9bc5-1cd3c7332dc7` | Rust S04 fixture/schema parity/TS parity/typecheck raw0; TS fixture raw1: unknown uint64 usage-counter format; зарегистрирован числовой unsigned format без coercion/unknown-format waiver |
| `a8338125-0dab-4539-b99f-aa0a5fc3fb70` | scoped fmt, Rust S04 fixture, Rust/TS generated parity, TS typecheck/S04 fixtures/boundary raw0; Python launch MISSING_EXECUTABLE/native_exit=null (relative executable path). Исправлен argv executable на existing resolved Python; это не PASS и не code-test FAIL |
| `6bf64991-db0b-4b25-b92b-39726f556bcb` | `python tests/contracts/check_schemas.py` raw0; boundary raw1: old host dependency/source allowlist. Явно добавлены host→sm-runtime, sole-writer processing_execution source и только zeroize fixture dev dependency |
| `14e91fc6-3f19-4d61-aff6-a813eb2da12e` | Python boundaries raw0; final affected default/seams clippy raw0; **единственный final-byte serial execution invocation**,4/4 passed115.05s, raw0; после feature test default production build всех восьми affected packages raw0 |
| `0790160b-2eba-444f-b5ba-12b2928948ba` | seams clippy с CLI/GUI consumers raw0; actual GUI TS consumer typecheck raw0; pinned rustfmt --check всех changed candidate .rs raw0; git diff --check raw0 |
| `757cc23e-011e-43cd-bb47-311c79616b32` | source guard: все12 явно записанных новых own package/library UUID roots отсутствуют; index empty. Никакого scan/delete неизвестных roots/PIDs |
| `0099ae41-7089-4b7a-a1d8-5d645d8bbcd9` | after-documentation git diff --check и git diff --cached --exit-code raw0; индекс пуст, commits нет |

Exact final execution argv:
`cargo test --offline --locked -p sm-host --features processing-test-seams --test processing_execution -- --test-threads=1 --nocapture`.
Four public-path matrices: load-intent SQL rollback/explicit retry/Ask/background/readback;
bounded32/serial-load1/fairness/pause/permission/profile/Off/uncertainty/reopen/expired held
lease; real61s reactor suspension with SQLite/backend still running and denied late
publication; real publication SQL rollback→Interrupted/reopen without SDK retry.
Это actual published SDK import, не mock Runtime trait. Real61s не заменяет OS sleep.

Default production build:
`cargo build --offline --locked -p sm-host -p sm-storage -p sm-runtime -p sm-application -p sm-session -p sm-protocol -p sm-cli -p sm-gui`.
Both feature paths use affected all-target `cargo clippy ... --all-targets -- -D warnings`;
seams invocation also includes CLI/GUI consumers. No all-features/global foundation reruns.

Shared execution fixtures: request Execution/Reconcile/background technical plan и
refused claim/backend-settled/lease-token authority; response ledger attempts/calls/model
leases/holds, technical metadata counts, rejected injected authority/unload/adopted state
and negative/fractional/overflow uint64. Rust, Ajv и Python draft2020-12 читают одни fixtures.
TS execution types/IDs экспортированы; stable storage/runtime/bootstrap/session owners и
AGENTS route обновлены. Serena unavailable (No language servers); Codebase exact callers
и actual ast_search подтвердили единственный boxed Processing producer в session.

## Следующее / честные остатки

- Writer не делал review/acceptance/commit; fresh parent acceptance — [result](result.md).
- GUI/CLI processing UX подключён последующим [interface owner](interfaces.md).
  S05 Analyze/OCR/effective/manual publisher — следующий scope; content_revision=None,
  record_revision не hard gate, manual merge отдельно.
- Authorized live LM Studio/selected model, real OS sleep, large-library RSS/latency,
  full regression/catalog, signed clean install и shipped runtime staging — NOT_RUN.
- Physical unload/other retention/reasoning modes — Unsupported; native TTL requested900,
  verification=not_verified не означает physical retention/unload proof.
- SDK/bootstrap/foundation accepted receipts не повторялись; immutable0001–0005/девять планов не переписывались.

## Own resources / cleanup

Final four tests явно shutdown host, kill+wait только own synthetic WS Child handles
и remove fresh UUID roots; final logs теперь записывают own package UUID и Child PID.
Проверены ровно12 UUID из final logs:0 retained; source/package preparation cleanup
выполнен fixture lifecycle. Это не scan пользовательских данных и не GPU settlement proof.
Historical failure roots остаются ignored для evidence; их cleanup inventory/OS PID
confirmation **BLOCKED/NOT_RUN**: новых ownership records для старых PIDs нет. Native fixtures используют kill_on_drop=true; private SDK использует accepted owned ChildJob. Это не утверждение physical backend settlement.
Из последнего failed run известен retained own library `ec820816-e2fd-4b9d-956f-3fddad2103d1`; также diagnostic own library `c24bc145-1d62-486d-b80e-a1e68e2ede89`. Остальные exact UUIDs — строки `owned fixture library` в перечисленных native logs. Source/package UUID можно восстановить из import_batches.request_json **только этих собственных roots** после подтверждения закрытия owner; не сканировать/чистить чужие temporary roots. Process PIDs не записаны — не угадывать и не убивать по общей Node/image имени.
