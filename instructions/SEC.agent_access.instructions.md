---
applyTo: "crates/application/src/access.rs,crates/application/src/credentials.rs,crates/protocol/src/access.rs,crates/storage/src/access/**,crates/storage/src/access.rs,crates/storage/migrations/0009_agent_access.sql,crates/host/src/access/**,crates/host/src/access.rs,crates/session/**"
name: "SEC.AgentAccess"
description: "Читай при изменении persistent agent profiles/connections, metadata-only grants, exact credential enrollment/revoke, current authority и security-bound search cursors."
---

# SEC — persistent metadata-only agent authority

Источник — [ADR-012](../docs/adr/012-agent-access-core.md). Это authority/core S07,
не CLI/MCP, GUI access management или полный этап. Только `read_metadata`;
`AgentPolicy` требует ровно один grant, пустые roots/capture modes и mutation limit1.
Неподдержанные scopes/policies отвергаются до записи. Profile64/connection128 —
пределы всех persistent identities, не только активных. Name1…80 Unicode scalars;
UUIDv4 и decimal SQL revisions используют общий protocol validator.

## SQLite / credentials

`sm-storage::access` — порт прежнего bounded SQLite worker под исходным HostLease.
Additive0009 применяется только после прежнего root/DB preflight; 0001–0008,
library UUID, source и канон неизменны. Secret не хранится в SQL: только SHA-256
versioned CSPRNG secret и immutable enrollment identity. Profile versions,
receipts/events и фиксированные cleanup targets неизменяемы. Profile principal
создаёт worker; wire/client kind/название/claimed scopes его не назначают.
Нет второй auth DB/SQL/path authority в host или клиентах. Новые тесты — exact
UUID roots под `.pi/temporary`, без live upgrade/backup/destructive acceptance.

`CredentialKey::agent(environment, library, AgentConnectionId)` строит только
`<namespace>/library/<library>/agent/<connection>`; Owner targets не изменены.
`WindowsSecretStore` проверяет прежний Environment; delete_agent допускает только
exact agent key и подтверждает native absence. Нет enumeration/prefix sweep,
owner/backend fallback, token в JSON/config/argv/receipt/error/log.

`AccessServices` сначала ищет journal по owner+operation+fingerprint. Только новая
operation генерирует secret. Worker commits planned connection/digest ДО native
write; exact readback подтверждает digest, затем writer activation+receipt.
Replay использует те же IDs. Planned+missing не генерирует другой secret и явно
отказывает; оператор может отозвать незавершённое connection и создать новую
operation. Different/invalid slot не усыновляется/не перезаписывается. SQL и vault
не общая транзакция. Fault seams доступны только `cfg(test)`.

Revoke terminal; disable/re-enable повышает security revision и не восстанавливает
старые contexts/cursors. Authority сначала COMMIT/publish, exact cleanup затем;
cleanup failure не возвращает права. Profile cleanup использует durable targets
исходной operation: её replay не удаляет новые connections после re-enable.

## Authentication / delivery

Application1.0 negotiated16 capabilities включает `access.request`; Owner получает
явный `ManageAccess`, TestReadOnly остаётся status/doctor-only, backend не session.
ApplicationClient загружает выбранный exact agent key после trusted server image
проверки. Private Hello передаёт connection identity, не principal/scopes. Прежние
8KiB/единый absolute3s handshake и bounded transport admission сохраняются.

`AgentAuthority` читает durable state при handshake и каждом request. Context
содержит connection/security_revision/credential_generation. Search window и
fingerprint ledger также связываются с этим binding; cancel/page чужого connection
отказываются даже при общем profile principal. Source ANY-required-scope проверяется
до top-k/snippet/page/card. Global maintenance category catalog для agent явно
отказывает, Owner поведение сохраняется; legacy ListRecords не расширяется.

Publication boundary — shared storage-owned RwLock. Owned write permit передаётся
в admitted writer command и живёт до COMMIT/ответа даже при drop caller. Session
проверяет authority после delayed handler и держит read DeliveryPermit через
реальный framing send. Поэтому revoke COMMIT и выдача упорядочены, а не основаны
на immutable verifier snapshot. Сам Storage Knowledge/Search также проверяет
current authority перед возвратом. Уже отправленные bytes отозвать невозможно.
Отзыв не останавливает writer, owner capture, другие connections, SDK/jobs/model.

## Проверки и cleanup

Focused offline/locked: `cargo test -p sm-host -p sm-storage -p sm-credentials
-p sm-protocol s07_ -- --test-threads=1`, affected session/knowledge tests, owner
clippy/build, Rust-generated schemas и TS/Ajv/Python shared fixtures.
Native exact Test slot проверяется отдельно от injected store faults. Guards
вооружаются сразу после successful write; exact cleanup/absence подтверждается
до удаления synthetic root. Worker/readers и own server tasks закрываются раньше
root. Raw failures не перезаписываются; source/index/immutable SQL inventory и
SOURCE_RELEASED обязательны перед handoff. Scope/остаток — в
[access-core evidence](../docs/progress/S07/access-core.md).
