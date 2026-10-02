# scripts/ — developer preview and verification commands

## Offline verification (current foundation subset)

```powershell
powershell.exe -NoProfile -NonInteractive -File scripts/verify.ps1
powershell.exe -NoProfile -NonInteractive -File scripts/verify-contracts.ps1
powershell.exe -NoProfile -NonInteractive -File scripts/test-verify.ps1
```

These never start the product host: they run the offline contract subset only. Since the
working host/CLI slice, `verify.ps1` also runs `HOST-COMPOSITION` (`cargo test -p
sm-host`) and `CLI-SURFACE` (`cargo test -p sm-cli`) against in-process compositions, and
since the diagnostic window slice it builds the React assets first
(`GUI-FRONTEND-BUILD`), then runs `GUI-RUST-TEST` (`cargo test -p sm-gui`),
`GUI-FRONTEND-TYPECHECK`, `GUI-FRONTEND-LINT` and `GUI-FRONTEND-TEST`. The frontend build is
required because the window crate embeds `apps/gui/dist`; it uses only the pinned local Node
and the already installed `apps/gui/node_modules`. The offline suite deliberately never opens a
window — the window runtime is reported as the explicit `GUI-WINDOW-RUNTIME` **NOT_RUN**
catalog entry in `result.json`.

Prerequisites must already exist: project-local Rust 1.98.1, Node 24.13.0,
pnpm package 10.32.1, frozen local node_modules, MSVC and Python/jsonschema 4.26.0.
The runner does not install or download anything. Cargo and nested contract commands
inherit `CARGO_NET_OFFLINE=true`; verify/preview share the separate
`.pi/staging-target-t09b`, never another running host's old `.pi/preview-target`.

**Desktop capture is not offline.** `sm-capture`'s `live_capture` integration target
freezes the whole current desktop (not just synthetic HWNDs). It requires the explicit
non-default `isolated-desktop-tests` feature. Never enable it or run all-features tests
on a personal session. `CAPTURE-NATIVE` checks safe library tests only;
`CAPTURE-LIVE-DESKTOP` stays NOT_RUN for the separate owned-isolated S02/S08 harness.
`test-offline-safety.ps1` proves metadata/default denial with compile-only Cargo exit 101;
it never enables the feature or dispatches live capture. No stack or test suppression
is needed by the current baseline.

## Diagnostic window preview (separate from verification)

```powershell
powershell.exe -NoProfile -NonInteractive -File scripts/preview-gui.ps1 -Cleanup
```

One owned `.pi\preview\<uuid>` root, one explicit trusted configuration and the real
`shotd.exe`/`shot.exe`/`screenmemory-gui.exe` binaries; the frontend assets and the binaries are
built with the pinned project-local toolchain into their own unique private build-log roots.
The default `-Action selfcheck` drives the **same** window client headlessly; each binary
prints exactly one JSON value (CLI exit codes), while the wrapper saves immutable evidence
and prints progress. It verifies that a repeated check attaches to the same host
instance and stops the owned host with a checked exit. `-Action window` really opens the Tauri
window on that owned root, closes it like the user's close button and proves that the same
owned `shotd.exe` process outlived the window and was reused by the reopened check.
Other options: `-PreviewRoot <owned preview dir>` to reuse a prepared root, `-SkipBuild` to use
existing binaries, `-WindowHoldSeconds <n>` (default 2) to hold the window before the
agent closes it, `-OutputRoot <new .pi path>` for immutable evidence and `-Cleanup` to
remove the synthetic root and exact Test credential. A finally block closes the owned
window through its held handle. Privileged host/credential/root finalization is armed only
by a fully validated init-issued binding, before connect. Rejected preflight never cleans up;
changed config/binding refuses finalization and preserves the local host/root/credential.
Unchanged inputs retain normal error cleanup. No user click is required.
`window` also records a private physical PNG using only PrintWindow of its verified
owned HWND (DPI-aware, no desktop capture).
This is an explicit development preview, **not** a production first-run installer and **not**
part of `verify.ps1`: no personal root, model, capture or network access is involved. Clean up
its owned root (and its synthetic Test credential) with
`scripts/preview.ps1 -Action cleanup -PreviewRoot <owned preview dir>`.

## Owned-preview cleanup safety suite (mandatory)

```powershell
powershell.exe -NoProfile -NonInteractive -File scripts/test-preview-safety.ps1
```

Executable negative tests for the cleanup guards on private synthetic UUID paths only (no
personal data, no model, no network, no live credential): a foreign directory, a directory
outside `.pi\preview`, a missing or mismatched ownership marker, shell metacharacters in
`library_id` (with a canary file that an injection would have created), a live owned
`shotd.exe`, an unverifiable stop, a reparse point inside the owned tree (including a junction
below the earlier depth bound, `root/data/a/b/link`, pointing at an external synthetic sentinel),
an over-limit tree and every non-1168 credential readback verdict must all be
refused with a non-zero exit **and nothing deleted**, while each sentinel file survives. The
native `CredReadW` readback is exercised for real both for an absent target (`ERROR_NOT_FOUND`)
and for a freshly created synthetic credential (`success`) before it is deleted, and the
readback-verdict seam is unset again after every substituted case. The
suite is called by `scripts/test-verify.ps1`, so it is part of the mandatory verification
path; it writes its own new UUID output root and never leaves a synthetic credential behind.

## Developer preview (one owned test root, real binaries)

```powershell
powershell.exe -NoProfile -File scripts/preview.ps1
```

This builds `shotd.exe` and `shot.exe` with the pinned project-local toolchain, prepares a
new owned root under `.pi\preview\<uuid>`, initializes its library, enrolls the synthetic
`ScreenMemory.Test.V1` owner credential, then drives the real CLI:

- artifact paths: `.pi\staging-target-t09b\debug\shotd.exe` (host), `.pi\staging-target-t09b\debug\shot.exe` (CLI);
- trusted configuration: `.pi\preview\<uuid>\preview.json` (explicit absolute root,
  library id and host image; no CWD/ENV/PATH fallback);
- the one command the script prints is `shot.exe --config <preview.json> status --json`.

Other actions: `-Action status|doctor|stop|prepare` (reuse `-PreviewRoot`),
`-SkipBuild` to use the existing binaries, and

```powershell
powershell.exe -NoProfile -File scripts/preview.ps1 -Action cleanup -PreviewRoot <preview dir>
```

`cleanup` acts **only** on a directory this preview created: a direct child of `.pi\preview`
with a canonical UUIDv4 name and an ownership marker holding exactly that UUID, an explicit
trusted configuration with a canonical `library_id`, an independent create-only
`.screenmemory-preview-library.json` binding issued by this root's actual `shotd --init`,
matching directory/library/config/root/schema/test/image, no reparse point anywhere in the **whole**
owned tree (every directory is listed non-recursively and each entry's attributes are tested
before the entry is used as a path, so a junction at any depth is refused rather than followed)
and at most an explicit 10000-element inspection limit. Missing/legacy/mismatched binding
is refused without adoption. The validated tuple and config/binding hashes are rechecked
before privileged actions; mutation preserves the root and credential. These developer
helper checks are not an atomic sandbox against hostile same-user file replacement.
It then stops the owned host with a checked exit, waits (bounded) for its own `shotd.exe` to be
gone, deletes exactly the owned `ScreenMemory.Test.V1/library/<id>/owner` target by invoking
`cmdkey.exe` directly with an argument list (never through a shell), proves absence **only**
through the native `CredReadW` readback (`ERROR_NOT_FOUND` is the only accepted absence; a still
existing credential or any other Win32 code refuses) and removes the owned root. Any foreign,
similar-looking, unmarked, tampered, still-running or
otherwise unverifiable root is refused with a safe message and a non-zero exit **without
deleting anything**; the owned root and the exact credential are kept for reconciliation.
This is an explicit development preview, **not** a production first-run installer: no personal
root, model, capture or network access is involved. Every run allocates a new UUID directory
for its own private build log under `.pi\preview-build-logs\<uuid>\build.log`; the shared
mutable `.pi\temporary\preview-build.log` is no longer written, so preview and smoke runs are
always repeatable without overwriting another run's evidence.

## Explicit synthetic executable smoke (separate from verification)

```powershell
powershell.exe -NoProfile -NonInteractive -File scripts/smoke-host-cli.ps1
```

Starts only the freshly built `shotd.exe`/`shot.exe` on one owned `.pi` root, and asserts:
first-run and reopen library identity, CLI `status`/`doctor` consistency on one owner,
duplicate-start refusal, `--no-start` without any side effect, owner stop with an actual
process/endpoint end, restart with the same library id, no child process/AI state, and
exact synthetic credential cleanup. Evidence is written to an immutable new `.pi` output
root (`result.json` plus per-step logs); it is deliberately **not** part of `verify.ps1`.
The current storage migration schema is 4; the preview configuration schema remains 1.

## Composed foundation regression (owned binaries/window, no desktop capture)

```powershell
powershell.exe -NoProfile -NonInteractive -File scripts/test-foundation-smoke.ps1
```

Runs host/CLI smoke, GUI selfcheck, an automatically closed real Tauri window with
physical owned-HWND evidence/reopen, then injected failures after connect and window
spawn, plus `test-preview-rejection.ps1`: 12 cases on real initialized roots A/B and
an already-running B. It covers config/library/root/image/schema/env substitution,
direct CLI cleanup, missing/malformed/mismatched bindings and mutation after arm.
Unchanged-input failures must return raw 1 and still prove window/host gone, exact Test
credential absence and owned-root removal. Mutation must instead preserve the host/key/root
with explicit local BLOCKED; only the harness restores and finalizes its own fixtures. Every run uses new UUID roots/evidence;
this is separate from offline verify. SDK/FTS5/package checks remain S04/S06/S08
NOT_RUN, embeddings/vector checks are excluded by ADR-011, and S01 stays IN_PROGRESS.
