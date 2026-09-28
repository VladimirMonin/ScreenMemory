# S01-T05d — working `shotd` + `shot` (host/CLI integration)

**S01 IN_PROGRESS; independent review pending.** Base `bfb2f486e843d89b0b9a415dfa1305cb86514e97`
(shared candidate tree with the S01-T06c session slice). Scope: real host composition and
the thin CLI client over the accepted session/application-client, plus the first developer
preview root. No accepted pipe/auth/framing/storage primitive was rewritten; GUI/logon/
remote/live-SDK/packaging stay outside this slice. Nine source plans unchanged. Contract:
[ADR-009](../../adr/009-host-cli-composition.md), [HOST.composition](../../../instructions/HOST.composition.instructions.md).
The milestone review `.pi/orchestration/checkpoints/t05d-host-cli/milestone-review-312173dc.json`
kept this candidate and asked for four narrow fixes (preview-cleanup ownership and `cmdkey`
safety, per-run preview build logs, credential-before-logger bootstrap order, two documentation
claims); they are implemented in the same candidate and recorded in
[Review-fix delta (Sol6 milestone review)](#review-fix-delta-sol6-milestone-review).

## What is now real

- `sm-host` → `shotd.exe`: explicit absolute `--root`, `LibraryRootResolver` (Test
  namespace only, fail closed for anything else), `HostLease` **before** SQLite, the
  consumed lease owned by the single `StorageWorker`, explicit Test-namespace owner
  enrollment **before** the bounded logger (S01 §6.3 step 5: credential policy, then
  structured logs and minimal services), real `FoundationApi`/`FoundationControlApi`, sealed
  `Listener`, `host.ready` published only after the bind, AI off and bridge stopped.
  `shotd --init` performs first-run library creation/enrollment of an already prepared
  owned root and prints one bootstrap JSON (`library_id`, `schema_version`, `format_major`).
- `sm-cli` → `shot.exe`: `--version`, `status [--no-start] --json`, `doctor --json`,
  `host stop --json`; one JSON value on stdout, diagnostics on stderr, exit codes 0/2/4/6/7/8.
  It never opens SQLite, never links the storage owner and never resolves a root/image from
  CWD/ENV/PATH: one explicit absolute `--config` file (`schema_version`, `environment=test`,
  absolute `root`, canonical `library_id`, absolute `host_executable`).
- Autostart: at most one launch of the configured fixed image with an absolute root, then
  bounded 100 ms retry/discovery for 5 s; credential/identity errors are never retried; only
  this CLI's own unreached child can be terminated; `--no-start`/`host stop` never start a
  host and create no lock/database/log side effect.
- Shutdown: owner `request_shutdown` returns `state=stopping` (not "handles closed"), stops
  admission, gives admitted connections a bounded 750 ms drain so an in-flight authorized
  receipt is still written, closes the listener, confirms SQLite closure (lease released only
  then) and flushes the logger inside the 5 s budget. A plain CLI exit never stops the host.
- Host stdio: a host that outlives the CLI must not hold the caller's pipes, so the CLI
  detaches its own standard handles before the spawn (Windows inherits every inheritable
  handle, not only the configured stdio) and captures safe pre-root host diagnostics into
  `<root>/cache/host-stderr.log`, truncated per launch.
- `doctor` reads live components: root identity + held lease, credential store readback,
  storage schema/identity, real SQLite runtime, log sink (owned probe file plus the logger's
  sink counter) → `PASSED`; `foundation.resource_manifest`/`bridge.supervisor` →
  `UNSUPPORTED`; `runtime.lmstudio`/`codec.webp_avif`/`index.fts_vec` → `NOT_RUN`. A failed
  mandatory check exits 7 for the CLI; an absent optional runtime does not fail the foundation.
- Developer preview: `scripts/preview.ps1` builds with the pinned project-local toolchain into
  its own new UUID build-log directory, prepares one owned `.pi\preview\<canonical uuidv4>` root
  with an ownership marker holding exactly that UUID, initializes it, writes the trusted config
  and prints the single check command. `-Action cleanup` accepts **only** such an owned root
  (direct child of `.pi\preview`, canonical UUIDv4 name, matching marker, canonical-UUID
  `library_id`, no reparse point anywhere in the **whole** owned tree, at most the explicit
  element limit); it stops the owned host with a checked exit,
  waits boundedly for its own `shotd.exe` to be gone, deletes exactly the owned
  `ScreenMemory.Test.V1/library/<id>/owner` target by invoking `cmdkey.exe` directly with an
  argument list (never through a shell), proves absence **only** through the native
  `CredReadW` readback (`ERROR_NOT_FOUND`; success or any other code refuses) and only then
  removes the root.
  Anything foreign, similar-looking, unmarked, tampered, still running or otherwise unverifiable
  is refused with a safe message and a non-zero exit, and **nothing is deleted**.
  Artifacts: `.pi\preview-target\debug\shotd.exe`, `.pi\preview-target\debug\shot.exe`.

## Minimal integration hook in the accepted session

`sm-session` gains `run_draining(listener, host, stop, drain)`; `run` is now the same loop
with `Duration::ZERO`, so no accepted behaviour changed. On stop the loop stops accepting and
gives already admitted connections a bounded window to finish an in-flight response before
aborting their tasks, which is what makes a `stopping` receipt reachable. New native test
`s01_host_cli_stop_drain_preserves_in_flight_response` holds a delayed handler, signals stop,
asserts the loop stays in its drain (not aborting the task) and then receives the typed
response inside the window. `crates/session/tests/native_session.rs` grows from 4 to 5
native cases; the 4 wire cases are unchanged (SESSION-NATIVE step: 5 native + 4 wire, all PASS).

## Checks actually run (raw exits, private artifacts)

| Check | Command | Raw exit | Artifact |
|---|---|---|---|
| HOST-COMPOSITION | `cargo test --locked -p sm-host --offline` (4/4 at that time; 5/5 after this report's review fix) | 0 | `.pi/temporary/host-cli-test-host-3.log` (4-test run), `.pi/review-fixes-dffcefe5-0bd8-42fe-8db9-0216a8257a8e/sm-host.log` (5/5) |
| CLI-SURFACE | `cargo test -p sm-cli --offline` (5/5, lock updated once without `--locked`) | 0 | `.pi/temporary/host-cli-test-cli-5.log` |
| SESSION-NATIVE | `cargo test --locked -p sm-session --offline -- --test-threads=1` (5 native + 4 wire) | 0 | `.pi/temporary/host-cli-test-session-4.log` |
| RUST-FMT | `cargo fmt --all -- --check` | 0 | green source: the `RUST-FMT` step (native exit 0, empty output) of `.pi/verify-240ab4bf7b2545839ef2ce66bd344eed/result.json`; `.pi/temporary/final-fmt.log` is an earlier RED artifact (rustfmt diff in `crates/cli/src/launcher.rs`) kept as-is |
| RUST-CLIPPY | `cargo clippy --workspace --all-targets --locked -- -D warnings` | 0 | `.pi/temporary/final-clippy2.log` |
| RUST-TEST | `cargo test --workspace --locked` (37 suites ok) | 0 | `.pi/temporary/final-ws-test.log` |
| BOUNDARIES | `python tests/contracts/check_boundaries.py` | 0 | `.pi/temporary/final-contracts.log` |
| VERIFY | `scripts/verify.ps1` outer, 26 steps PASS on the final tree (`HOST-COMPOSITION`, `CLI-SURFACE` added by this slice) | 0 | `.pi/verify-240ab4bf7b2545839ef2ce66bd344eed/result.json` |
| CONTRACTS | `scripts/verify-contracts.ps1` outer, 15 steps PASS | 0 | `.pi/contracts-6ac313bb56914fcdad2889c754127f1e/result.json` |
| TEST-VERIFY | `scripts/test-verify.ps1` outer (fixed argv inventory extended, integrity/exit7 RED paths kept) | 0 | `.pi/temporary/final-testverify.log` |
| SMOKE | `scripts/smoke-host-cli.ps1` (explicit executable smoke, 53 PASS steps = 42 assertions + 11 executable steps) | 0 | `.pi/smoke-host-cli-374a31c0808d4740bd7b5a1e202236a0/result.json` |
| PREVIEW | `scripts/preview.ps1` then `-Action cleanup` | 0 | `.pi/temporary/host-cli-preview-run-2.log` |
| HELPER-ISOLATION | `.pi/temporary/exit-isolation-probe.ps1` (bounded helper isolation) | 0 | `.pi/temporary/exit-isolation-26136b4dc87f42edad4db8d63eb979d4/result.json` |

Earlier same-code runs `.pi/verify-98c27a121d7544cda54bd019dba78bb5/result.json` and
`.pi/verify-74ed539c75574346b765c76a6fe23b4f/result.json` (both outer 0) were taken before
this report was written; the cited run above is the last one and covers the complete tree. The first full verify of the slice failed only on `SOURCE-TEXT` because two files written through a CRLF
text-mode round trip (`crates/cli/Cargo.toml`, `scripts/preview.ps1`) had CRLF; both were
normalised to LF and the rerun above is clean (`.pi/temporary/final-verify.log` keeps the RED).

Smoke evidence (`result.json`, sanitized args, per-step stdout/stderr logs) proves on the
built images: fresh identity then reopen with the same `library_id`, CLI `status`/`doctor`
agreeing on one `host_instance_id`, duplicate `shotd --root` refused with exit 4 and no
second writer, `--no-start` absent host → one error object + exit 4 with no lock/database/
process side effect, owner stop with the process and endpoint actually gone, restart with a
new host instance and the same library, no child process of the owned host, and exact
synthetic credential cleanup (delete then absence verification). No model, capture, network
or personal data is involved; the boundary suite additionally proves no model/SDK/HTTP crate
is linked at all.

## Exit-capture and command-line isolation of the preview helper

The shared helper (`scripts/preview-common.ps1`) was isolated on a fresh private UUID output
root with a bounded probe after an earlier wrapper run reported `exit=1 reason=OK`: a native
child exit 0 stays the integer 0 (never `$null`, never coerced `$false`), a native exit 7 stays
7 and is never read as success, a quoted argv with spaces survives, a child filling both pipes
(20000 lines each) is fully drained without deadlock, a never-exiting child is bounded to
`exit=$null reason=TIMEOUT`, a missing image is `exit=$null reason=MISSING_EXECUTABLE`, and a
wrapper under a path containing spaces propagates its own 0/7.

The same probe found and fixed a real invocation hazard: `cmd.exe` parses its own command line,
so the C-runtime style per-argument quoting used for ordinary executables corrupts inner quotes
(the literal `\"` reaches cmd). The build helper now passes a verbatim command line
(`Invoke-PreviewProcess -RawArguments`) in the `/s`-safe form `""<path>""`, which also survives a
checkout path containing spaces. Every probe attempt used its own new private output root and no
cited log was overwritten; the production build path was re-validated afterwards (smoke below).

## Review-fix delta (Sol6 milestone review)

The milestone review kept the candidate tree and named four blockers; each one is fixed here
without reworking or rolling back the accepted 49-path delta (no accepted framing/session/
storage logic was touched).

| Fix | What changed | Proof on real files |
|---|---|---|
| A HIGH — preview cleanup | `cleanup` acts only on a directory this preview created: canonical path must be a direct child of `<repo>/.pi/preview`, the directory name must be a canonical UUIDv4, an ownership marker (`owned-preview.id`, created only by `New-OwnedPreviewRoot`) must hold exactly that UUID, the `library_id` must be a canonical UUID and the owned tree must contain no reparse point. A missing/mismatched marker, a foreign parent, a tampered marker, a reparse point or a non-canonical id refuses with a safe message and a non-zero exit, deleting nothing. Before any delete: a checked `stop`, a bounded `Wait-OwnedShotdGone` and an absence check of this root's own `shotd.exe`; the exact synthetic `ScreenMemory.Test.V1/library/<id>/owner` target is deleted by invoking `cmdkey.exe` directly with an argument list (the `cmd /c` shell path is gone), and the root is removed only after that absence is confirmed and ownership is re-proven. | `scripts/test-preview-safety.ps1`: 40/40 PASS, exit 0 — `.pi/preview-safety-888a3b64-5ec8-46aa-a280-afb6f10320c7/result.json` (also run inside `test-verify` as `.pi/test-verify-2e3ce2eaed8f4c7c81dff87372961f71/preview-safety/result.json`); **extended to 62 checks by the focused fix below**. Live end-to-end: `preview.ps1 -Action prepare|status|cleanup` on `.pi/preview/f81c3216-0797-4c92-bea9-5101e3beadfb` (library `5e114d38-86d4-4fe6-8c33-30674bd5b9b1`) → `stage=removed stop_exit=0 credential_stage=verified`, exit 0, no `shotd.exe` left |
| B MEDIUM — shared build log | `Build-PreviewBinaries` allocates a new UUID directory before the build (`.pi/preview-build-logs/<uuid>/build.log`), refuses an existing path and never writes `.pi/temporary/preview-build.log` again; preview/smoke output roots stay per-run UUIDs, so both commands are freely re-runnable. | new log `.pi/preview-build-logs/77c8db5c-8ca1-47ad-be0c-784ec798b248/build.log` (smoke exit 0); `.pi/temporary/preview-build.log` untouched (size 89, not modified by this run) |
| C MEDIUM — bootstrap order | `crates/host/src/bootstrap.rs` enrolls/reopens the owner credential **before** `BoundedLogger::start` (S01 §6.3 step 5 order); listener and the rest are unmoved, error/secret semantics unchanged. A refused enrollment now returns exit 6 without ever creating a file logger. | new composition test `s01_host_cli_05_owner_enrollment_precedes_the_logger` (`cargo test -p sm-host`: 5/5, `.pi/review-fixes-dffcefe5-0bd8-42fe-8db9-0216a8257a8e/sm-host.log`) |
| D LOW — report claims | `final-fmt.log` is now named as the earlier RED artifact whose green source is the `RUST-FMT` step of the cited `verify.ps1` run; the smoke counter is unified to 53 PASS steps (42 assertions + 11 executable steps) in both report files. | rows corrected in this file and in `t05d-host-cli.json`; both cited smoke result files really contain 53 steps |

Checks re-run after the fixes (all raw exits 0, artifacts private):

| Check | Artifact |
|---|---|
| `python tests/contracts/check_boundaries.py` | `.pi/review-fixes-dffcefe5-0bd8-42fe-8db9-0216a8257a8e/contracts-boundaries.log` |
| `cargo test --locked --offline -p sm-host|sm-cli|sm-session` (5/5, 5/5, 5 native + 4 wire) | `.pi/review-fixes-…/sm-host.log|sm-cli.log|sm-session.log` |
| `cargo fmt --all -- --check`, `cargo clippy -p sm-host --all-targets -D warnings` | `.pi/review-fixes-…/fmt-check.log`, `.pi/review-fixes-…/clippy-host.log` |
| `scripts/smoke-host-cli.ps1` (53/53 PASS, new binaries) | `.pi/smoke-host-cli-8f2420b39f6746bdae99aeba43f419f0/result.json` |
| `scripts/preview.ps1` prepare/status/cleanup on a fresh root | `.pi/review-fixes-…/preview-prepare.log|preview-status.log|preview-cleanup.log` |
| `scripts/verify.ps1` (26/26 PASS) | `.pi/verify-1e57aa4fb6044591b2804cb0bbfb4b3e/result.json` |
| `scripts/verify-contracts.ps1` (15/15 PASS) | `.pi/contracts-a75d72dd5f904a1daaf5343a62aee1fb/result.json` |
| `scripts/test-verify.ps1` (now runs the safety suite as a mandatory step) | `.pi/review-fixes-…/test-verify.log`, safety evidence above |

## Focused-confirmation delta (fada888c)

The focused confirmation
`.pi/orchestration/checkpoints/t05d-host-cli/focused-fix-confirmation-fada888c.json` confirmed
B/C/D and the rest of the cleanup guards, and left exactly two HIGH items of fix A open. Both
are fixed here; no other part of that delta was reworked and no Rust product code was touched.

| Fix | What changed | Proof on real files |
|---|---|---|
| HIGH-1 — whole-tree reparse refusal | `Test-OwnedPreviewTreeSafe` no longer stops at depth 3: an explicit `System.Collections.Stack` walk lists every directory non-recursively (`Get-ChildItem` without `-Recurse` lists a reparse point itself instead of descending through it, also on Windows PowerShell 5.1), tests each entry's `Attributes` for `ReparsePoint` **before** that entry is ever used as a path and never follows a link. Any reparse point anywhere in the tree, an enumeration error, or a tree above the single explicit element limit (`Get-PreviewTreeElementLimit` = 10000) refuses as `reparse`/`tree-read`/`size` with **nothing deleted**. | `.pi/probe-7c1f2a54-2e9b-4a11-9c8d-1f0b2a6d4e77/deep-junction-delta-probe.ps1` (raw exit 0): a junction at `root/data/a/b/link` → external synthetic sentinel gives `old_depth_bounded_walk_safe=True` for the previous algorithm and `new_whole_tree_walk_safe=False reason=reparse counted=8`; the external sentinel was intact after the case removal. Suite steps `DEEP-JUNCTION-CREATED/REFUSED/PRESERVED/TARGET-PRESERVED`, `TREE-INSPECTION-DEFAULT-ACCEPTS` (counted=8, limit=10000), `TREE-ELEMENT-LIMIT-REFUSED` (reason=limit counted=3), `TREE-ELEMENT-LIMIT-PRESERVED` |
| HIGH-2 — unambiguous credential readback | The second `cmdkey /delete` heuristic is gone. After the exact delete (still a direct `cmdkey.exe` invocation with an argument list, never a shell), absence is proven only by a minimal `Add-Type` P/Invoke `advapi32.CredReadW(target, CRED_TYPE_GENERIC=1, 0)` with `CredFree` on success: `0` (the credential still exists) refuses, `GetLastError()==1168 ERROR_NOT_FOUND` is the only accepted absence and every other code refuses as `absence-error`; the credential blob and user name are never read or printed. A test-only script-scope seam substitutes the readback verdict for the safety suite; production never sets it. | probe `.pi/probe-7c1f2a54-…/credread-probe.ps1` (raw exit 0): `add_type_compiled=True absent_code=1168 add_exit=0 present_code=0 delete_exit=0 after_delete_code=1168`. Suite steps `CREDENTIAL-NATIVE-READBACK-ABSENT`/`EXISTS`, `CREDENTIAL-READBACK-EXISTS-REFUSED`+`PRESERVED` (stage=credential-absence), `CREDENTIAL-READBACK-ABSENT-ACCEPTED` (stage=removed, credential_stage=verified), `CREDENTIAL-READBACK-ERROR-2/5-REFUSED`+`PRESERVED` (stage=credential-absence-error, code kept), `CREDENTIAL-SEAM-RESTORED`, and all four synthetic targets proven absent with the real readback afterwards |

The suite also drops its own junction links with `[System.IO.Directory]::Delete($link, $false)`
instead of `Remove-Item -Force`, because a non-interactive PowerShell host cannot answer the
confirmation prompt for a junction; probe `.pi/probe-7c1f2a54-…/junction-delete-probe.ps1`
(raw exit 0) shows that this call removes only the link, that `Remove-Item -Recurse -Force` on a
directory containing a junction does not follow it, and that `Remove-Item -Force` without
`-Recurse` fails on a junction in a non-interactive host.

Checks re-run after the focused fix (all raw exits 0, private artifacts, new UUID roots). The
table cites the second pass, run on the **final bytes** after the document-only edits below;
the first pass (before those edits) produced the same outcomes and the same 62/62 safety count:

| Check | Artifact |
|---|---|
| `scripts/test-preview-safety.ps1` (62/62 PASS, 0 notes) | `.pi/test-preview-safety-7c63a8e2e6be4562b293ff2b81318899/result.json` |
| safety suite as the mandatory step of `scripts/test-verify.ps1` (62/62 PASS) | `.pi/test-verify-65852be17d574b6b87058d10f73b1fc0/preview-safety/result.json` |
| `scripts/smoke-host-cli.ps1` (53/53 PASS, new binaries) | `.pi/smoke-host-cli-c9d61abc1439415d8841b68483c00ea4/result.json` |
| `cargo test --locked --offline -p sm-host, -p sm-cli, -p sm-session -- --test-threads=1` (5/5, 5/5, 5 native + 4 wire) | `.pi/probe-7c1f2a54-…/sm-host.log`, `sm-cli.log`, `sm-session.log` |
| `python tests/contracts/check_boundaries.py` | `.pi/probe-7c1f2a54-…/boundaries.log` |
| `scripts/verify.ps1` (26/26 PASS, final bytes) | `.pi/verify-8cfb98abc1f54858ab6ff20dbde9e79d/result.json` |
| `scripts/verify-contracts.ps1` (15/15 PASS) | `.pi/contracts-f5336abe27e54f4ea1a532c8f8e1096e/result.json` |
| `scripts/test-verify.ps1` (outer exit 0, final bytes) | `.pi/probe-7c1f2a54-…/test-verify-final.log` |
| live `scripts/preview.ps1 -Action cleanup -SkipBuild` on a real owned root whose own `shotd.exe` was still running (`stage=removed stop_exit=0 credential_stage=verified`, exit 0) | `.pi/probe-7c1f2a54-…/preview-cleanup-bounded.log` |

Live end-to-end on the current revision: an interrupted ad-hoc probe left one owned synthetic
root `.pi/preview/165566fb-8137-4161-b179-06c4e07f556d` (library
`f8767952-8cbf-46cb-a6fd-fa49580925c8`, its own `shotd.exe` still running). The product cleanup
path then stopped that host (`stop_exit=0`), deleted the exact target, confirmed absence through
the native readback (`credential_stage=verified`, code 1168), removed the owned root and exited 0;
an independent direct `cmdkey /delete` afterwards reported raw exit 1 and the readback stayed
`absent`/1168, with no `shotd.exe` left. Observation reported for the host/CLI owner (not fixed
in this delta): that ad-hoc probe read the redirected pipes with an unbounded `ReadToEnd` and did
not return while the spawned host was alive; the sanctioned harnesses use the bounded
`Invoke-PreviewProcess`/`Invoke-PinnedCargo` helpers and are unaffected (smoke 53/53, this
bounded cleanup exit 0).

The smoke run proves the native readback end to end on the real built host: the exact synthetic
`ScreenMemory.Test.V1/library/93af45bc-15ce-4219-9935-5d73e3d7bcfe/owner` credential was
enrolled by `shotd --init`, deleted by the helper and confirmed absent by the readback, and a
separate direct `cmdkey /delete` afterwards reported "Element not found" with raw exit 1.

Documentation-only edits never touched the nine immutable plans, and no historical RED artifact
(`final-fmt.log`, the early CRLF `SOURCE-TEXT` RED, the failed-first attempts) was overwritten.

## Not accepted / not implemented

Actual GUI integration, production first-run root provisioning, other logon/remote/restricted
token gates, full IPC/HOST stage case catalog (crash/UNC/alias), runtime inventory, live
probe/consent, codecs, FTS/vec, bridge, packaging and whole-S01 acceptance remain
`NOT_RUN`/`NOT_IMPLEMENTED`. `verify.ps1` still reports the coarse stage gates `HOST`/`IPC`
as not implemented: those labels are the full stage-owned acceptance, not this slice.

## Residual risks

1. The accepted session's forced-abort path (a connection task aborted without any drain)
   leaves that connection's pipe instance alive and an in-flight client request pending until
   the client disconnects; observed while probing the plain `run` stop, mitigated in the
   product flow by the host's bounded drain, and reported for the session owner rather than
   fixed here.
2. Typed log delivery is bounded/best-effort (`emit` uses a try-lock): under contention a
   single INFO event (for example `host.ready`) may be dropped while the readiness itself is
   functional state; `doctor` reports the logger's sink counter, not a delivery guarantee.
3. The CLI has no local per-request timeout (the host enforces `timeout_ms`); a host dying
   mid-request without closing the pipe is bounded only by the drain.
4. Autostart discovery is capped at 5 s: a genuinely slower host start is terminated as an
   owned unreached child and must be restarted by the next command.
5. `<root>/cache/host-stderr.log` is created by the CLI (truncated per launch) and is not part
   of the host's own rotation policy.
6. Preview cleanup requires ownership evidence that roots created by the pre-fix script do not
   have (ownership marker, canonical UUIDv4 directory name). Such a root is now refused instead
   of deleted; it must be reconciled manually, and only a root created by the current preview is
   removable by `-Action cleanup`.
7. Cleanup refuses whenever the owning `shot.exe` image is unavailable or cannot stop the owned
   host: that is deliberate (no unverified delete), so an operator without a built
   `.pi\preview-target` image must remove nothing but reconcile the root by hand.
8. A raw `System.Diagnostics.Process` reader that drains a redirected pipe with an unbounded
   `ReadToEnd` did not return while the spawned owned host was alive; the sanctioned harnesses
   use the bounded `Invoke-PreviewProcess` helper, so they are unaffected. Reported to the
   host/CLI owner as a stdio observation, not fixed in this slice.
