# scripts/ — developer preview and verification commands

## Offline verification (unchanged contract subset)

```powershell
powershell.exe -NoProfile -NonInteractive -File scripts/verify.ps1
powershell.exe -NoProfile -NonInteractive -File scripts/verify-contracts.ps1
powershell.exe -NoProfile -NonInteractive -File scripts/test-verify.ps1
```

These never start the product host: they run the offline contract subset only. Since the
working host/CLI slice, `verify.ps1` also runs `HOST-COMPOSITION` (`cargo test -p
sm-host`) and `CLI-SURFACE` (`cargo test -p sm-cli`) against in-process compositions.

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

- artifact paths: `.pi\preview-target\debug\shotd.exe` (host), `.pi\preview-target\debug\shot.exe` (CLI);
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
trusted configuration with a canonical `library_id`, no reparse point anywhere in the **whole**
owned tree (every directory is listed non-recursively and each entry's attributes are tested
before the entry is used as a path, so a junction at any depth is refused rather than followed)
and at most an explicit 10000-element inspection limit.
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
