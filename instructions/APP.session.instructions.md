---
applyTo: "crates/session/**,crates/application/src/lib.rs,crates/framing/src/lib.rs,scripts/verify.ps1,scripts/test-verify.ps1,docs/progress/S01/t06c-session.*"
name: "APP.Session"
description: "Читай при работе с authenticated native RPC session, application-client, method authorization, cancellation и bounded dispatch."
---

# APP — Authenticated RPC session subset

Источник: S01 §§5.1–5.5, 6.3, 10; [ADR-008](../docs/adr/008-session-client.md). `sm-session` принимает **уже OS-проверенный** `sm-pipe::Connection`: 16 pre-sealed native handles и trusted client-side server image из `SEC.pipe_endpoint`. Host composition после HostLease/DB/explicit SecretStore enrollment вызывает `SessionHost::new` с настоящими `FoundationApi`/`FoundationControlApi` services, shared `BufferBudget` на весь host и host/library IDs из validated root/metadata; затем `run` или `serve`. В тестах допустим injected delayed handler, не production fake report.

Handshake — private Rust serde DTO, не bridge/application method schema: fixed application 1.0 и двенадцать capabilities, 8 KiB, **единый абсолютный** 3s deadline на чтение/verifier/ответ. Client знает expected library из уже разрешённой конфигурации и вызывает `ApplicationClient::connect(endpoint, trusted_image, library, expected_host, credential_source, budget)`; сначала OS image check, затем explicit scoped `CredentialSource`. Нет enrollment в session, host library не переназначается по wire. Owner/read-only выбирается только точным verifier host policy, `client_kind`/params не разрешают права. В обычных ошибках нет raw frame/secret/path.

После handshake — DTO только из `sm-protocol`, строгие envelope/known params; scopes до handler. Только status, обычный doctor и request_shutdown направляются typed ports; runtime inventory и остальные известные методы получают METHOD_NOT_SUPPORTED после scope check. Reader держит отдельный bounded pump (cancel поллингуемого `Reader` poison); max 8 active IDs/connection до фактической отправки terminal response, девятый request при полном admission **закрывает соединение** без вызова handler. Одновременные writer sends ограничены `sm-framing` восемью. Unknown/duplicate IDs не запускают handler. Cancel ищет только ID текущего connection; disconnect signals+aborts waiting tasks, не backend stop и не host shutdown. Нет автоматического retry mutating operation. S03 import/export идут в optional
ImageLibraryApi, Owner ImportFiles/ExportImages scopes проверяются до dispatch;
TestReadOnly остаётся status/doctor-only. `library.asset.preview` — ReadImages,
`library.record.metadata` — отдельный ReadSourceMetadata, не неявный доступ к GPS.
Listing вызывается через foundation source-aware view. Legacy asset.read остаётся
canonical, preview имеет отдельный MIME/digest derivative contract.

Client — общий Rust adapter для CLI/private Tauri; `status`, `doctor`, `doctor_cancellable`, `request_shutdown` возвращают проверенные typed результаты; autostart/root selection/output formatting остаются integration owner. Не выдавать session subset за завершённый HOST/IPC/S01. Native tests: `cargo test --locked -p sm-session`; mandatory offline step `SESSION-NATIVE` плюс workspace/contract suite. Только новые owned Test roots и in-memory synthetic credentials; никакого native vault enrollment в session tests.
