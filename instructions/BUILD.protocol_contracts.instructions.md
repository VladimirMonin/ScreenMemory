---
applyTo: "Cargo.toml,Cargo.lock,rust-toolchain.toml,crates/domain/**,crates/protocol/**,crates/application/**,schemas/**,tests/fixtures/protocol/**,tests/contracts/**,package.json,.npmrc,pnpm-lock.yaml,pnpm-workspace.yaml,packages/protocol/**"
name: "BUILD.ProtocolContracts"
description: "Читай при изменении Rust DTO/ports, JSON Schema, общих fixtures, TS protocol consumer, генерации или pnpm contract scripts."
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

TS consumer `packages/protocol` генерирует типы закреплённым
`json-schema-to-typescript` из четырёх схем Rust owner. `node
packages/protocol/scripts/generate.mjs --output <new-dir>` создаёт только новую
директорию; `pnpm contracts:check` сравнивает bytes с checked-in TS без записи.
После явной проверки можно опубликовать generated файлы. Проверяй Ajv2020
`strict: true` с `ajv-formats`, локальными `uint16/uint32`, без coercion,
removeAdditional, defaults и remote refs. Типы не заменяют runtime validation:
для application request нужны и envelope, и `(method,params)`, bridge — отдельная
версия/схема. Ошибки валидатора не отдавай вызывающему коду вместе с payload.
Pinned Node 24.13.0 и pnpm 10.32.1; `pnpm install --frozen-lockfile
--ignore-scripts` после просмотра lifecycle, затем `pnpm typecheck`, `pnpm lint`,
`pnpm test`, `pnpm contracts:check`. Один package — protocol; пока frontend/bridge
не существуют, нельзя обещать их проверку.

На Windows используй закреплённый `rust-toolchain.toml` (MSVC 1.98.1),
локальный toolchain/cache и process-only VS developer environment, без
глобальной установки. После появления локальных компонентов того же release:
`cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets --locked -- -D warnings`,
`cargo test --workspace --locked`, `cargo run --locked -p sm-protocol --example
generate_schemas -- --check schemas`. Дополнительно `python
tests/contracts/check_boundaries.py`; `python tests/contracts/check_schemas.py`
требует явно `jsonschema==4.26.0`, не ставит пакеты автоматически.
Не запускать donor/app/SDK ради проверки DTO. После S01-T04a единый offline
subset запускается `scripts/verify.ps1`, отдельный bounded regeneration gate —
`scripts/verify-contracts.ps1`; их prerequisites, output и статус см.
[BUILD.verification](BUILD.verification.instructions.md). Сохраняй raw exit codes и
проверяй девять source-plan hashes отдельно; transport/host — будущие срезы,
Rust schema/fixtures и TS consumer не подтверждают их исполнение.
