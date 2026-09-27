---
applyTo: "Cargo.toml,Cargo.lock,rust-toolchain.toml,crates/domain/**,crates/protocol/**,crates/application/**,schemas/**,tests/fixtures/protocol/**,tests/contracts/**"
name: "BUILD.ProtocolContracts"
description: "Читай при изменении Rust DTO, доменных ID/revision, application ports, JSON Schema или общих protocol fixtures."
---

# BUILD — Protocol contracts

Rust `sm-domain` → `sm-protocol` → `sm-application` — единственный owner типов
первого foundation API. `sm-domain` не зависит от IO/OS, protocol от host/SDK,
application объявляет порты без адаптеров. Версии application и private bridge
отдельны, сейчас обе 1.0. Scope и `RequestContext` создаёт только будущая
аутентифицированная host-граница; wire `client_kind` не даёт полномочий.
Контракт и отложенные части перечислены в [ADR-002](../docs/adr/002-foundation-protocol.md).

Перед изменением wire добавь Rust positive/negative test и синтетический JSON
fixture. При изменении ограничений/enum перегенерируй из `sm-protocol` schemas
через `cargo run --locked -p sm-protocol --example generate_schemas -- --output
<new-project-local-temp-dir>` (директория должна отсутствовать), сравни с
`schemas/`, затем опубликуй согласованную версию. `--check schemas` только
сравнивает байты в памяти с файлами и не перезаписывает их. Схемы должны
оставаться draft 2020-12; `application-methods-v1` отдельно проверяет
`method`+`params`, envelope сам по себе не доказывает допустимость метода.

На Windows используй закреплённый `rust-toolchain.toml` (MSVC 1.98.1),
локальный toolchain/cache и process-only VS developer environment, без
глобальной установки. После появления локальных компонентов того же release:
`cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets --locked -- -D warnings`,
`cargo test --workspace --locked`, `cargo run --locked -p sm-protocol --example
generate_schemas -- --check schemas`. Дополнительно `python
tests/contracts/check_boundaries.py`; `python tests/contracts/check_schemas.py`
требует явно `jsonschema==4.26.0`, не ставит пакеты автоматически.
Не запускать donor/app/SDK ради проверки DTO. Сохраняй raw exit codes и
проверяй девять source-plan hashes отдельно; TS/Ajv и transport/host — будущие
срезы, Rust schema/fixtures сами по себе не подтверждают их исполнение.
