# S02-T09m3 — pure intent/reopened-v2 equality gate (candidate)

Baseline: HEAD `d0ec5459` with uncommitted T09m3 candidate; no schema, host, codec, startup/status/retry or commit path changes. The original candidate accepted a public mutable DTO and did not prove its claimed SQLite timestamp provenance. Correction: `sm-storage::match_recovery::match_pending_reopened_v2` borrows a sealed `DbValidatedIntent` returned only from `StorageWorker::read_validated_capture_intent` (SQLite worker, existing scoped Saved-first `intents::read`) and a separately validated/owned `ReopenedV2`. A match requires v2, exact IDs/mode/geometry/frame, pixel hash and byte length, context semantic equality and the digest of the intent's canonical JSON, captured time/profile, ordered receipt warnings (including empty), automatic provenance and the fixed path-policy discriminator. The returned private view borrows both values and exposes immutable pixels/context/warnings, captured time and `planned_created_at_utc` from the SQLite witness only; it cannot choose a path or commit. This is a read-time DB snapshot, not a reservation; DB may change later. The application `PendingCaptureIntent` DTO and repository trait remain unchanged, but cannot be passed to the public matcher; internal pure equality stays available only to unit tests. A refusal is generic typed mismatch/integrity and never deletes, repairs, writes, encodes or yields Saved. Existing v1 remains unrecoverable. This is **not** production-wired recovery.

Synthetic tests `s02_t09m3_01/02` use unique UUIDv4 `.pi/temporary` roots; create intent before staging the same packed RGBA; shutdown/reacquire, read pending and reopen v2; verify successful exact and empty-warning match, every compared field and warning ordering/empty mismatch, v1 rejection, foreign reopen/tampered pixel refusal, unchanged stage files, no records/assets/journal and `lookup_operation=Unknown`. Fixture teardown applies only to its own completed synthetic root, not user libraries.

## Original candidate checks (before sealed-witness correction; pinned Rust 1.98.1 MSVC; offline cache/process-local VS environment)

Initial verification wrapper invocation failed before tests (raw 1: native `cmd /s` quoting); corrected process-local VS environment invocation, no toolchain change. First boundary and contract probes after implementation failed raw 1 because the source-file allowlist omitted the two new modules; allowlist and pure-matcher assertions were updated. No failing product test was hidden.

Final after all source and test changes:

| Command | Raw exit | Result |
|---|---:|---|
| pinned rustfmt `--edition 2021` on scoped four Rust files; `--check` | 0 / 0 | scoped format PASS |
| pinned `cargo test --offline --locked -p sm-storage --lib s02_t09m3 -- --nocapture` | 0 | 2/2 PASS |
| pinned `cargo test --offline --locked -p sm-storage -p sm-isolation` | 0 | 105 storage tests; native isolation tests PASS |
| pinned `cargo test --workspace --offline --locked --quiet` | 0 | workspace PASS (one pre-existing ignored test) |
| pinned `cargo clippy --offline --locked -p sm-storage --all-targets -- -D warnings` | 0 | PASS |
| `python tests/contracts/check_boundaries.py` | 0 | PASS |
| `powershell.exe -NoProfile -NonInteractive -File scripts/verify-contracts.ps1` | 0 | implemented contract subset PASS; S01 still IN_PROGRESS; plan/source guards included |
| `git diff --check`; `git diff --cached --name-only` | 0 / 0 | no whitespace errors, no staged files |

Logs/wrapper stay private in `.pi/temporary`; verify-contracts generated its own new immutable `.pi/` output. No live root, donor, model, network, stage, commit or push. These earlier passing results apply to the original candidate, **not** to the corrected bytes. Independent read-only re-review ultimately returned scoped PASS for the sealed-witness correction (not for this original candidate). Recovery orchestration, Saved transition, path construction, startup enumeration, retry/status, process-kill and power-loss durability are **NOT_WIRED/NOT_RUN**.

## HIGH correction evidence (current uncommitted bytes)

`DbValidatedIntent` is non-Clone, has a private field and no public constructor or mutable accessor; its only creation site is `read_validated` after scoped `intents::read` on the SQLite worker. `ValidatedIntentRead::Pending(Box<DbValidatedIntent>)` avoids the large-enum clippy diagnostic. The application repository trait and open DTO remain unchanged. The public matcher exclusively takes `&DbValidatedIntent`; two Rust `compile_fail` doc tests reject a mutable DTO argument and direct witness construction. The internal DTO equality helper remains private, exercising field-negative cases without exporting a forgery path. Exact match after worker shutdown/reacquire, forged planned month on the public DTO (unchanged witness), empty warnings, conflict/unknown, wrong own stage, v1 and tamper refusals remain covered in synthetic roots. No records/assets/saved journal are written in this test.

Pinned MSVC Rust 1.98.1, offline, process-local VS environment and isolated `.pi/` target; each line reports its actual raw exit:

| Check | Raw exit | Scope/result |
|---|---:|---|
| `cargo test --offline --locked -p sm-storage --lib s02_t09m3` | 0 | 2/2 |
| `cargo test --offline --locked -p sm-storage -p sm-isolation` | 0 | 105 storage tests, isolation native suite, 2 compile-fail doctests |
| `cargo test --workspace --offline --locked --quiet` | 0 before final own-stage test, then 127 | latter run: two unrelated `sm-codec` parallel roundtrip stack overflows; not a PASS for final bytes |
| `cargo test --offline --locked -p sm-codec --test roundtrip -- --test-threads=1` | 0 | 6/6 exact native codec roundtrips |
| `cargo test --workspace --offline --locked --quiet -- --test-threads=1` | 0 | final bytes: serialized workspace PASS; 1 pre-existing ignored |
| `cargo clippy --offline --locked -p sm-storage --all-targets -- -D warnings` | 101 then 0 | large enum initially rejected, boxed Pending witness and passed |
| scoped `rustfmt --edition 2021 --config skip_children=true --check` (3 storage files) | 0 | scoped bytes formatted |
| `cargo fmt --all -- --check` | 1 | unrelated pre-existing formatting in application/cli/gui/host/session; not edited here |
| `python tests/contracts/check_boundaries.py` | 0 | sealed witness and worker route asserted |
| `powershell.exe -NoProfile -NonInteractive -File scripts/verify-contracts.ps1` | 0 | implemented contract subset PASS, S01 remains IN_PROGRESS |
| `git diff --check`; `git diff --cached --name-only` | 0 / 0 | no whitespace errors; no staged files |

The transient clippy failure preceded boxing; storage/isolation, serialized workspace and scoped Clippy pass on final source bytes. The default-thread workspace failure is unresolved and must remain visible: codec parallel stack overflow did not reproduce with serialized codec/workspace (including the parent's final-byte serialized run, exit 0), and codec was not edited. The contract runner and boundary script reran on the final own-stage mismatch test bytes. Workspace fmt failure is outside this bounded change and was not auto-formatted. No host startup/reconciliation, Saved, path derivation, retry/status, process-kill or power-loss proof was executed or claimed; acceptance remains with independent reviewer.
