# ADR-010 — Diagnostic window as one more client of the shared session client

Status: **S01-T06d candidate; independent review pending**. Sources: S01 §§4.1–4.2, 5.3,
10.1, 10.3; prior [ADR-007](007-native-pipe.md), [ADR-008](008-session-client.md),
[ADR-009](009-host-cli-composition.md). Nine immutable source plans unchanged. Evidence:
[S01-T06d](../progress/S01/t06d-gui.md), [t06d-gui.json](../progress/S01/t06d-gui.json).

## Decision

The S01 diagnostic window is a **second integration client**, not a second host and not a
second protocol implementation. `crates/gui` (`sm-gui`, binary `screenmemory-gui`) is a Tauri
v2 shell whose five typed commands are thin adapters over one `GuiSession`; `GuiSession`
reuses the same `sm_session::ApplicationClient` and the same integration primitives `shot`
uses (`sm-cli::connect_session`, shared `ClientFailure`, refusal messages and the exit-code
table). The window never speaks the wire protocol, never opens the library database, never
links the storage owner and cannot stop the host.

The frontend (`apps/gui`, Vite + React + TypeScript) is a real built workspace whose only
runtime imports are React, `@tauri-apps/api/core`, local modules and the *types* of the
generated protocol contract. It has no SDK, SQLite, native capture binding, node builtin,
`fetch`/`WebSocket`/`XHR` or dynamic import, and it can only name commands that really exist in
`crates/gui/src/shell.rs`; both properties are checked by `apps/gui/scripts/check-boundaries.mjs`
plus a parity check against the Rust owners (`check-parity.mjs`).

## Trusted configuration

The window accepts exactly one explicit absolute local `--config <file>` with the CLI
dev-preview schema (`schema_version = 1`, `environment = test`, absolute `root`, canonical UUID
`library_id`, absolute `host_executable`). There is no default location and no CWD/ENV/PATH/HOME
lookup, and UNC/device/ADS spellings are refused before any file-system access. A refusal is a
bounded safe message plus exit 2, emitted before any window or webview is created. Production
first-run and user roots are out of this slice; the shell runs only against an owned
`.pi/preview/<uuid>` root.

## States and honesty rules (S01 §10.1/§10.3)

Six states: `connecting` (one attempt bounded by `bounded_ms = 5000` from the shared client
constants), `ready`, `runtime_unavailable` (AI policy enabled but the bridge is not ready — this
must not fail the whole application), `protocol_mismatch` (major/capability/format refusal; no
status is guessed), `root_credential_error` and `host_not_running`. `state_for_failure` is a
total mapping of the published `ErrorCode` set, and an intentionally disabled AI policy stays
`ready`.

The diagnostic envelope carries no count, path or model field. Capabilities that do not exist in
this slice are rendered from the owner's `NOT_IMPLEMENTED` list as «не реализовано на этом
этапе»; an empty result is never shown as a real zero ("0 изображений"). An unfinished doctor
check is never presented as passed, and a failed mandatory check makes the command non-zero.

Closing the window closes only its connection: the host keeps running and a reopened window
attaches to the same `host_instance_id`.

## Headless proof and the offline suite

`screenmemory-gui --self-check --config <file>` performs the same connect → status → doctor
sequence without a window, prints exactly one JSON value on stdout and returns the CLI exit
codes with a bounded safe line on stderr. The offline verification suite never starts a window:
it builds the frontend assets (they are embedded into the crate), runs `cargo test -p sm-gui`,
the frontend `typecheck`/`lint`/`test`, and reports the window runtime as an explicit
`GUI-WINDOW-RUNTIME` **NOT_RUN** catalog entry naming `scripts/preview-gui.ps1 -Action window`.
Only that script starts the real window, always on an owned `.pi/preview/<uuid>` root with an
explicit trusted configuration and a unique private build-log root.

## Toolchain and supply chain

`tauri`/`tauri-build` are pinned to `2.10.3`/`2.5.6` and resolved from the project-local cargo
cache, so `cargo build|check|test --offline` works without network access. The frontend is built
with the pinned `.pi/toolchains/node-v24.13.0-win-x64` and the already installed local
`apps/gui/node_modules` (no global install, no download). `apps/gui/dist` is a git-ignored build
output, therefore the frontend build runs before any cargo step that compiles the window crate.
A missing WebView2/CMake/network prerequisite is reported as BLOCKED with the exact error; the
technology is never substituted.

## Consequences

- The window cannot drift from the CLI: both go through the same client, the same trusted
  configuration and the same refusal table, and a parity test compares the window's status/doctor
  payloads with `shot status --json`/`shot doctor --json`.
- The GUI adds a large dependency tree to the workspace. It is accepted because the window is a
  required V1 surface, and the tree is pinned, offline-resolvable and confined to `crates/gui`.
- `sm-cli` gained four `pub` items (shared failure type, connect helper, config message, target
  failure) purely as the reuse seam; the CLI's own behaviour and exit codes are unchanged.
- The workspace package allowlists are explicit two-entry lists, not wildcards, so a future
  package cannot silently join the verified workspace.
