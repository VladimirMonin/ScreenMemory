# S02-T09n3a — isolated recovery canonical file lane (writer evidence)

Baseline clean HEAD `8a50bf7` (n1 capability committed at `02dc02b`). Only this n3a file-only slice; nine original plans, `images::PublishedFile`, migrations, host and wire unchanged. Candidate is the non-Clone codec-verified n2 value from a sealed SQLite pending witness and owned v2 stage; no RGBA copy or re-encode during publish.

`StorageWorker::publish_recovery_candidate` uses existing single-slot file FIFO/admission and borrowed original `HostLease::canonical_root`. The month/record plan is derived exclusively from the private validated SQLite witness (not stage or retry clock). New fixed `assets/YYYY-MM/<record-v4>/original.webp` writes the encoded bytes to a UUIDv4 attempt temp, syncs, publishes no-replace, then opens a bounded pinned no-follow handle; existing final never gets overwritten/deleted/adopted by name and goes straight to bounded reopen. Both cases compare exact length, SHA-256 and bytes against the candidate. Failures retain temp/final. Sealed non-Clone `VerifiedPublished` retains candidate/witness and pinned handle, with crate-private bounded nonconsuming revalidation. `CanonicalRead` now also owns the already validated NTFS record-directory handle including its complete assets/month/record ancestor pin chain (otherwise a file handle alone would permit parent-directory rename). These pins block ordinary rename, **not** same-user SHARE_WRITE mutation: future SQL transaction must revalidate pending and canonical file at its commit boundary. This does not itself grant SQL write or Saved.

Synthetic exact UUIDv4 `.pi/temporary` tests `s02_t09n3a_01..03` exercise pending→v2 stage→new publish; shutdown/reacquire→existing final (no extra temp); simulated fault after publication/before hypothetical BEGIN leaves pending + manifest + final, zero records/assets/saved; same-length different-digest final remains untouched; too-large final refused; junction month and hardlinked final refused without affecting sentinel; assets/month rename attempts fail while a returned published handle lives; mutation of pinned same-length final makes later revalidation fail. Existing isolation tests cover missing/unsafe no-follow open and no-replace collision; missing final *during publication/reopen* race not fault-injected. No live data, models, clipboard, donor, network or host startup exercised.

Checks on final code bytes (process-local MSVC Rust 1.98.1 helper; serialized native):

| Command | Raw exit | Result |
|---|---:|---|
| `.pi/t09a-run.cmd test --offline --locked -p sm-storage --lib s02_t09n3a -- --test-threads=1` | 0 | 3/3 |
| `.pi/t09a-run.cmd test --offline --locked -p sm-storage -p sm-isolation -p sm-codec -- --test-threads=1` | 0 | storage 110, isolation native and codec incl. doc tests |
| `.pi/t09a-run.cmd test --workspace --offline --locked --quiet -- --test-threads=1` | 0 | serialized workspace incl. storage 110, four compile-fail docs; one preexisting ignored test |
| `.pi/t09a-run.cmd clippy --workspace --all-targets --offline --locked -- -D warnings` | 0 | PASS |
| scoped `rustfmt --edition 2021 --config skip_children=true --check` (four changed Rust files) | 0 | PASS |
| `.pi/t09a-run.cmd fmt --all -- --check` | 1 | pre-existing formatting in untouched application/cli/gui/host/session; no widening |
| `python tests/contracts/check_boundaries.py` | 0 | PASS on final contract bytes |
| `powershell.exe -NoProfile -NonInteractive -File scripts/verify-contracts.ps1` | 0 | implemented contract subset PASS; S01 remains IN_PROGRESS |
| `git diff --check`; `git diff --cached --name-only` | 0; 0 | no whitespace errors; no staged paths |

Full F06/F07 process-kill/power-loss, RSS <=1 GiB, host retry/startup/SQL Saved and atomic DB/file transaction are NOT_RUN/NOT_WIRED. Independent review/acceptance and commit belong to integrator.
