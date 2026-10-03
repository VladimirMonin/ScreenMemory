---
applyTo: "crates/application/src/credentials.rs,crates/credentials/**,crates/application/tests/credentials.rs,crates/host/src/runtime_config.rs,crates/host/src/main.rs"
name: "SEC.Credentials"
description: "Читай при изменении session/backend enrollment, bounded native stdin, exact Windows Credential Manager authority, host профилей и секретных handshake-байтов."
---

# SEC — локальные credentials перед secure listener

S01 §5.4 и [ADR-005](../docs/adr/005-local-credentials.md) задают границу: `sm-application::credentials` объявляет `SecretStore`, ключи и host policy; `sm-credentials` — единственный Win32 Credential Manager adapter. `Environment` из `sm-isolation` обязателен для native adapter; target строится только из своего namespace, typed library UUID и фиксированного profile. Никакого произвольного target, enumeration, Inspector vault, ENV/settings/plaintext/DPAPI-file fallback. TestReadOnly — только явное synthetic Test enrollment, не схема выдачи MCP-токенов.

Host сначала удерживает библиотечный `HostLease` и выполняет **явный** enrollment; обычная аутентификация только проверяет уже загруженный verifier. Любой missing/invalid/Win32 failure даёт отказ, не privileged context. `CredWriteW` не поддерживает atomic create-if-absent: pre-read предотвращает обычное случайное overwrite, exclusive library lease обязателен для своего host, злонамеренный same-user process не изолирован. Не логировать secret bytes, не передавать их в argv/URL/params/исключения; read-only kind=gui не получает owner scopes. Native peer ACL/framing и session handler gate реализованы отдельными S01-T06b/T06c subset; host bootstrap/actual services, полный IPC/other-logon acceptance всё ещё НЕ реализованы.

S04 fixed `CredentialKey::runtime_backend(environment, library)` хранит только backend
UTF-8 token1…2048 bytes в `<namespace>/library/<id>/lm-studio`, не session credential.
Profile имеет пустые scopes; ApplicationClient отказывает этому profile до vault read.
Owner/read-only blobs остаются versioned33 bytes. `shotd --root <own-root>
--enroll-runtime-token` принимает только nonterminal bounded stdin до EOF ≤10 s
(без newline/control bytes; timeout exit8 до root/vault authority), удерживает HostLease
и create-new exact key; нет overwrite/enumeration,
argv/ENV/plaintext settings fallback. HostSecretAuthority читает только этот exact key;
missing/invalid отказывает SDK start. Guest допустим только явно в host-owned runtime
config: это guest именно backend, а application session остаётся authenticated Owner.
Токен не поступает в WebView/localStorage/logs. Native synthetic cleanup подтверждает
absence обоих созданных exact targets после остановки собственных resources.

Нативные тесты допускаются только с новым уникальным UUID под `ScreenMemory.Test.V1` и синтетическим значением: exact target collision → refusal без delete; cleanup только созданного точного target; при ошибке report bounded target identity и операцию без bytes. Guard живёт до начала fallible enrollment, вооружается сразу после успешного native write (не после readback) и на unwind удаляет только собственный exact target; collision не передаёт ownership. После удаления подтвердить native absence (`CredReadW` → `ERROR_NOT_FOUND`); `CredDeleteW` success отдельно не доказывает отсутствие. При отказе guard сообщает раздельно `delete`/`absence verification`, непосредственный Win32 code при native failure и точную безопасную identity без blob/bytes. Не скрывать ошибку, не повторять native/full проверки до точечной reconciliation и не делать prefix sweep. Проверки: обязательные `CREDENTIALS-POLICY`, `CREDENTIALS-NATIVE` в `scripts/verify.ps1`, contract boundary test и новый output root под `.pi/`; `verify-contracts.ps1` отдельно. Stage остаётся `IN_PROGRESS`, HOST/IPC/SEC01 NOT_RUN. Не распространять тестовый cleanup на живые credentials.
