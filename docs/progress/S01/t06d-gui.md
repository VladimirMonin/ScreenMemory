# S01-T06d — minimal working diagnostic window and headless self-check (candidate)

**S01 IN_PROGRESS; no independent review yet.** Base `e5cecff1fba77269a26eeeb438a2cd8126f12c02`.
Scope: one real Tauri v2 diagnostic window over the *already accepted* shared `sm-session`
client, its typed command surface, the explicit trusted configuration, the headless
`--self-check` and the developer-preview script. Not in scope: capture, gallery, search,
drop zone, model settings, the AI bridge, MCP, packaging/installer, production first-run.
Nine source plans unchanged. Contract: [ADR-010](../../adr/010-diagnostic-window.md),
[APP.gui](../../../instructions/APP.gui.instructions.md); machine evidence:
[t06d-gui.json](t06d-gui.json).

This slice continued an interrupted work-in-progress tree: the unfinished `crates/gui` and
`apps/gui` sources were first **built and tested**, then completed where they were missing.
The candidate was not discarded; the surviving parts are listed below.

## What really runs

- `crates/gui` (package `sm-gui`, binary `screenmemory-gui`) is a Tauri v2 shell with five
  typed commands (`gui_limits`, `gui_probe`, `gui_connect`, `gui_doctor`, `gui_open_report`).
  Every handler is a thin adapter over one `GuiSession`, i.e. over the same
  `sm_session::ApplicationClient` the CLI uses; no handler can name a path, a host image, a
  frame, a credential or a method, and no handler can stop the host.
- The window accepts exactly one **absolute local** `--config <file>` (the shared CLI
  dev-preview schema: `schema_version = 1`, `environment = test`, absolute `root`, canonical
  UUID `library_id`, absolute `host_executable`). There is no default path and no
  CWD/ENV/PATH/HOME fallback; a refusal is a stable safe message plus exit 2 before any window
  or webview exists.
- `--self-check` performs the same connect → status → doctor sequence headlessly, prints
  exactly **one** JSON value on stdout and returns the CLI exit codes (0/2/4/6/8) with a
  bounded stderr line. It is the executable proof of the window's client path on machines
  where a window cannot be inspected.
- The frontend is a real Vite + React + TypeScript workspace (`apps/gui`): connection state,
  build/protocol versions, host instance ID, library ID, schema, AI policy, AI runtime
  availability and bridge state; the buttons «Подключиться», «Проверить основу» and
  «Открыть диагностический отчёт» (the last one only ever opens the validated cache/log
  directory derived from the configuration). Unfinished capabilities are rendered as
  «не реализовано на этом этапе» from the owner list; no image counter, no invented zero.
- `scripts/preview-gui.ps1` is the only place that starts the real window: one owned
  `.pi/preview/<uuid>` root, one explicit trusted configuration, one unique
  `.pi/preview-build-logs/<uuid>` log root for the frontend build and one for the cargo
  build, and either `-Action selfcheck` (headless, default) or `-Action window`.

## States (S01 §10.1) and where they come from

`connecting` (one bounded attempt, `bounded_ms = 5000` from the shared client constants),
`ready` (real `HostStatus`), `runtime_unavailable` (AI policy enabled but bridge not ready —
never a whole-application failure), `protocol_mismatch` (major/capability/format refusal, no
guessed status is ever shown), `root_credential_error` (root/identity/credential refusal) and
`host_not_running` (no host answered and starting one was not requested or was not allowed).
`state_for_failure` is a total mapping of the published `ErrorCode` set onto these states; an
AI policy that is intentionally disabled is `ready`, not a failure. Closing the window closes
only this connection: the host keeps running and a reopened window attaches to the same
`host_instance_id`.

## Wiring

`crates/gui` is a workspace member with an exact dependency allowlist
(`sm-application`, `sm-cli`, `sm-credentials`, `sm-domain`, `sm-isolation`, `sm-protocol`,
`sm-session`, `serde`, `serde_json`, `tauri`, `tokio`, `windows-sys`; dev:
`sm-framing`, `sm-host`, `sm-pipe`, `tokio-util`, `uuid`) and its sources contain no
`rusqlite`, no storage owner, no `std::net`, no `Command::new` outside the report opener
(which resolves `explorer.exe` from `GetWindowsDirectoryW`, never from PATH). `apps/gui` is a
second pnpm workspace package with mandatory non-optional `typecheck`/`lint`/`test`/`build`
scripts. `verify.ps1` now runs `GUI-FRONTEND-BUILD`, `GUI-RUST-TEST`,
`GUI-FRONTEND-TYPECHECK`, `GUI-FRONTEND-LINT` and `GUI-FRONTEND-TEST`; the workspace allowlist
in `verification-common.ps1` and the protocol/TS boundary check were updated to accept exactly
`packages/protocol,apps/gui` instead of a wildcard. The offline suite never starts a window:
the window runtime is reported as an explicit `GUI-WINDOW-RUNTIME` **NOT_RUN** catalog entry
that names `scripts/preview-gui.ps1 -Action window`.

The `tauri`/`tauri-build` crates are pinned to `2.10.3`/`2.5.6` from the project-local cargo
cache; `cargo build/check/test --offline` succeeds without network access. The frontend is
built with the pinned `.pi/toolchains/node-v24.13.0-win-x64` and the already installed local
`apps/gui/node_modules` (no global install, no download). The window crate embeds
`apps/gui/dist`, which is git-ignored, so `GUI-FRONTEND-BUILD` runs before any cargo step that
compiles `sm-gui`.

## Checks actually executed (raw exits)

| Check | Command | Outcome |
|---|---|---|
| Cargo window suite | `cargo test --locked -p sm-gui --offline` | **0** — 2 unit + 9 native integration tests |
| Cargo clippy | `cargo clippy -p sm-gui --all-targets --locked --offline -- -D warnings` | **0** |
| Formatting | `cargo fmt --all -- --check` | **0** |
| Frontend typecheck | `pnpm --filter @screenmemory/gui typecheck` | **0** |
| Frontend lint + boundaries + parity | `pnpm --filter @screenmemory/gui lint` | **0** |
| Frontend tests | `pnpm --filter @screenmemory/gui test` | **0** — 9 vitest cases |
| Frontend build | `pnpm --filter @screenmemory/gui build` | **0** — real `apps/gui/dist` |
| Package boundaries | `python tests/contracts/check_boundaries.py` | **0** — thirteen crates |
| Workspace verify | `powershell -NoProfile -NonInteractive -File scripts/verify.ps1` | **0** — all steps PASS ([result](../../../.pi/t06d-verify/result.json)) |
| Verification harness | `powershell -NoProfile -NonInteractive -File scripts/test-verify.ps1` | **0** — real positive verify + contracts suites PASS (both include the new GUI steps, real harness root `.pi/test-verify-a2c14242a2364befad7501d9744012a8`) |
| Window self-check (real binaries) | `scripts/preview-gui.ps1 -Action selfcheck` | **0** — `ready`, real IDs, reopen = same instance, owned host stopped |
| Window runtime (real window) | `scripts/preview-gui.ps1 -Action window -WindowHoldSeconds 5` | **0** — window shown, same host pid outlived its close, reopened check reused the instance |

The headless self-check against a real owned host printed exactly one JSON line:
`mode=gui-self-check`, `exit_code=0`, `state=ready`, the configured `library_id`, a canonical
`host_instance_id`, `report_available=true`, `runtime_available=false`, the real foundation
doctor (storage/sqlite/lease/identity/credential/log checks `PASSED`, `runtime.lmstudio`
`NOT_RUN`, `codec.webp_avif`/`index.fts_vec` `NOT_RUN`, two `UNSUPPORTED`) and the owner list
of unfinished capabilities. A second self-check reached the *same* `host_instance_id`, and the
owned `shotd.exe` was stopped afterwards (`shot host stop` exit 0 and a verified process end).
In the window run the same owned host pid was observed before, during and after the window and
the reopened check attached to the same instance, which is the executable form of "closing the
window does not stop the host".

## Environment note (not a product defect)

During an earlier harness invocation an untracked junk file literally named `nul` appeared in
the repository root (its content was a POSIX shell error line, not repository content).
`Get-SourceSnapshot` then failed **closed** with `SOURCE_INVENTORY_ERROR`, i.e. the guard
refused to verify a tree whose inventory it could not read instead of reporting a false PASS.
The file was removed through the `\\?\` Win32 path (a reserved device name cannot be deleted
by an ordinary path), after which both `verify.ps1` and `test-verify.ps1` ran clean and did not
recreate it. A `shotd.exe` left over from an earlier session on the unrelated owned root
`.pi/preview/7482a571-…` was deliberately not touched, because it is not this run's owned
child; this run's own preview roots were stopped and cleaned up.

## Changed and reused from the interrupted tree

Reused as-is (after first compiling and running them): the explicit configuration/argument
parser and its refusals, the typed envelope and state mapping, the validated report opener, the
self-check contract and the session adapter. Completed or corrected:

- `apps/gui` gained its real workspace (Vite/React/TS sources, `model.ts` mapping, vitest
  cases, boundary and parity checkers); the checked-in placeholder `dist/index.html` is no
  longer used for anything, the directory is a build output.
- `crates/cli` changes were kept **because they are the reuse seam** (shared
  `ClientFailure`, `connect_session`, `config_message`, `target_failure`) and nothing else in
  `crates/cli` was touched; the same file was reformatted by `cargo fmt`.
- The forged-host fixture in `crates/gui/tests/gui_surface.rs` bound its synthetic pipe once
  and could fail intermittently (`PIPE_CREATE`/Win32 5) because the previous fixture's
  accepted connection kept the pipe name alive; it now closes its own connection handles and
  binds within an explicit bounded retry window.
- `crates/gui/src/selfcheck.rs` had a `clippy::collapsible_match` violation under
  `-D warnings`; the two `if let` levels are now one.
- `packages/protocol/scripts/check-boundaries.mjs` asserted that the workspace contains a
  single package; it now asserts the explicit two-entry allowlist.

## Not implemented / not accepted

Production first-run installation, real user-data roots, capture, gallery, search, drop zone,
model settings, the AI bridge, MCP, packaging/installer, autostart, and any full S01
acceptance. The offline suite does not start a window; the window runtime is only proven by
the explicit `scripts/preview-gui.ps1 -Action window` run recorded above. The window is not
driven headlessly (no UI automation): its commands are covered by the native tests over the
same `GuiSession` and by the headless `--self-check`, which is the same client sequence. A real
user's click path and visual layout are not asserted by any automated check. AI runtime,
codec and index checks stay `NOT_RUN`/`UNSUPPORTED`; the bridge has no live probe in this
slice. Whole S01 remains IN_PROGRESS and every other gate stays NOT_RUN.

## Reviewer handoff

Inspect: the exact `--config` validation (no implicit path), the command handlers' inability to
name a path/host/method, the bounded `bounded_ms`/retry behaviour, the state mapping totality,
the report directory validation (root resolver + link/reparse refusal), the frontend's
React/Tauri-only import surface and its command parity with `shell.rs`, the
`GUI-FRONTEND-BUILD`-before-cargo ordering in `verify.ps1`, and the two-entry workspace
allowlists. Not claimed: any production readiness, any live AI behaviour, any packaging, and
any claim that a human visually verified the window.
