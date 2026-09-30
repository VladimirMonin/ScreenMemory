# S02-T09n2 — sealed verified codec candidate (writer evidence)

Baseline HEAD `02dc02b0`, clean before first edit. Design source: read-only n2 section of `s02-t09n2-storage-candidate-design.md` (n3 excluded). This is an isolated candidate, **not accepted until independent review**. Changed: `crates/storage/{Cargo.toml,src/lib.rs,src/recovery_candidate.rs,src/match_recovery.rs,src/match_recovery_tests.rs,src/staging_files.rs}`, `crates/codec/src/lib.rs`, `tests/contracts/check_boundaries.py`, `instructions/DATA.storage_owner.instructions.md`, this report. Nine source plans untouched.

Sealed `Box<DbValidatedIntent>` from scoped SQLite read and existing owned v2 stage are consumed in the existing borrowed-lease file lane. `match_pending_reopened_v2` is checked there; intent encoder fingerprint must exactly equal current bundled `sm-codec` fingerprint; pixels Vec moves into `RgbaImage::from_rgba` without an additional RGBA allocation. Real WebP exact lossless encode/decode verification, including nonzero RGB at alpha 0, precedes SHA-256 of owned encoded bytes. Result is a non-Clone privately constructed `VerifiedCandidate` with immutable witness/IDs/geometry/fingerprint/encoded bytes/length/digest and redacted Debug (length/geometry only), not publication or Saved. Temporary **1,048,576 pixel** pre-encode recovery cap and **128 MiB encoded** cap; 96 MP normal recovery is NOT accepted. Native libwebp scratch allocation is not bounded by this API: strict global **1 GiB RSS NOT_PROVEN**. Existing file lane and stop admission retain original lease and FIFO; no new lane, SQL write or canonical file I/O.

Synthetic NTFS UUIDv4 root tests `s02_t09n2_01/02`: pending created before stage; shutdown/reacquire; real encoded alpha0 roundtrip and digest distinct from pixels; foreign owner, warning-order mismatch, obsolete encoder fingerprint, >1MP valid stage refusal, stage tamper refusal, unknown Saved lookup, empty records/assets/journal and absent canonical directory. Existing `s02_t09m3_01/02` additionally cover v1 rejection, context/IDs/hash/geometry/warning emptiness and foreign/tampered stages. Compile-fail doc probes reject public DTO as witness and candidate construction; their execution requires doctests (workspace run). 128 MiB encoded oversize cannot be produced with conservative pixel cap using available real codec fixtures; no fabricated container/header accepted as proof.

Checks (pinned MSVC Rust 1.98.1 via existing process-local VS helper, offline locked; serialized native):

| Command | Raw exit | Outcome |
|---|---:|---|
| `cargo test --offline --locked -p sm-storage --lib s02_t09n2 -- --test-threads=1` | 0 | 2/2 on final test content before enum boxing (same semantics; workspace rerun after) |
| `cargo clippy --offline --locked -p sm-storage --all-targets -- -D warnings` | 101 then 0; final 0 | large enum fixed by boxing reopened value; final PASS |
| scoped `rustfmt --edition 2021 --config skip_children=true --check` | 1 then 0 | final scoped files PASS |
| `cargo test --offline --locked -p sm-codec --test roundtrip -- --test-threads=1` | 0 | 6/6 PASS |
| `cargo test --workspace --offline --locked --quiet -- --test-threads=1` | 0; final 0 | final source PASS after Debug redaction; full serialized subset incl. compile-fail docs |
| `python tests/contracts/check_boundaries.py` | 0 | updated production codec dependency and storage module allowlist PASS |

| `powershell.exe -NoProfile -NonInteractive -File scripts/verify-contracts.ps1` | 0; final 0 | final code bytes PASS; S01 remains IN_PROGRESS; plan/source guards |
| `git diff --check`; `git diff --cached --name-only` | 2 then 0; 0 | trailing blank line in Cargo.toml fixed; no whitespace errors, no staged files |

No live root, donor, network, model, clipboard, shotd or Git staging/commit/push. N3 commit/publish, startup reconciliation, full F01–F10, process-kill/power-loss durability and 1 GiB RSS proof remain NOT_WIRED/NOT_RUN.
