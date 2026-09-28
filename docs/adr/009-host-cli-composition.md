# ADR-009 — Working host/CLI composition and the developer preview root

Status: **S01-T05d candidate; independent review pending**. Sources: S01 §§4.1–4.5, 6.1–6.5,
10.2–10.3, 11.5; prior [ADR-003](003-root-isolation.md),
[ADR-004](004-storage-worker.md), [ADR-005](005-local-credentials.md),
[ADR-007](007-native-pipe.md), [ADR-008](008-session-client.md). Nine immutable source
plans unchanged. Evidence: [S01-T05d](../progress/S01/t05d-host-cli.md).

`sm-host` (binary `shotd`) is the only composition root: it consumes an explicit absolute
root, owns the `HostLease`, the single SQLite writer, the credential policy, the bounded
logger and the sealed listener. `sm-cli` (binary `shot`) is a client of the already
accepted `sm-session` `ApplicationClient`; it never opens SQLite, never links the storage
owner and never selects another host implementation.

## Bootstrap order (S01 §6.3) and readiness

1. Bootstrap arguments are parsed without side effects; `--root <absolute local path>` is
   the only accepted root and `std::io::stderr` carries typed diagnostics before any file
   log exists.
2. `sm-isolation::LibraryRootResolver` resolves the four existing children of that root and
   refuses non-local, non-NTFS, reparse, overlap and unprepared trees; the endpoint identity
   is derived from the pinned open root.
3. `HostLease::acquire` takes the exclusive `host.lock` **before** SQLite; a busy lease is
   refused with exit 4 and never triggers a second writer or a PID/taskkill path.
4. The lease is consumed by `sm-storage::StorageWorker`; the schema snapshot provides the
   durable `library_id` and the schema version. The blocking worker handshake runs on a
   dedicated blocking thread, so the runtime thread only awaits typed storage replies.
5. Credential policy first: the owner credential is enrolled/reopened explicitly (Test
   namespace only) while the lease is held. Only then are the bounded JSONL logger and the real
   foundation services created. A refused enrollment therefore never creates a file logger; it
   is reported as a bounded typed event on stderr with the fixed policy exit code, and the
   successful-enrollment path is what makes the cache log sink exist at all.
6. `sm-pipe::Listener::bind` creates the 16 sealed instances, then `host.ready` is published
   and `serve` starts the bounded accept loop. AI is off and the bridge is not started: no
   model, SDK, capture or network call exists in this composition.

Bootstrap failure emits one bounded typed event and returns the fixed exit code
(2 usage, 4 host/root/lease/protocol, 6 credential/policy, 7 storage/logging).

## Shutdown (S01 §6.5)

An authorized owner `system.request_shutdown` returns a receipt with state `stopping` — the
request was accepted, **not** that handles are closed. The host then stops admission, gives
already admitted connections a bounded drain window (750 ms default) so an in-flight
authorized response can still be written, aborts the remaining session tasks, closes the
listener, confirms SQLite closure through the storage worker (which releases the pinned
entries and the lease only then), and finally flushes the bounded logger inside the 5 second
budget. A CLI exit never stops the host; only `shot host stop` requests it, and an
unconfirmed storage closure is reported as `unknown` rather than as `stopped`.

## Client-side trusted configuration

`shot` accepts one explicit absolute `--config <preview.json>` file with exactly:
`schema_version` (1), `environment` (only `test`), `root` (absolute local path), `library_id`
(canonical UUID) and `host_executable` (absolute local image). There is no default location,
no CWD/ENV/PATH lookup and no production/dev namespace in this slice: a foreign environment
is refused. The `library_id` comes from the host's explicit first-run `shotd --init` report
(`{"library_id": …, "schema_version": …, "format_major": …}`), which is what makes the
library-scoped credential target and the expected handshake identity unambiguous before the
CLI can authenticate.

## Commands, output and exit codes (S01 §10.2–10.3)

`shot --version`; `shot --config <cfg> status [--no-start] --json`; `shot … doctor --json`;
`shot … host stop --json`. Stdout carries at most one JSON value with no banner or progress
text; diagnostics go to stderr. Exit codes: 0 success, 2 arguments/configuration, 4
host/runtime, 6 permissions/policy, 7 processing/storage (including a failed mandatory
doctor check), 8 bounded wait. A `status --no-start` or `host stop` without a host returns
one safe error object and exit 4 without starting anything.

Autostart is allowed only for `status`/`doctor`: at most one launch of the configured fixed
absolute image with an absolute root argument, then bounded 100 ms retry/discovery for 5
seconds. A credential or identity failure is never retried. The CLI owns that child handle
only: it never terminates a foreign PID or an image name, and it terminates exactly its own
unreached child if the bounded window expires while that child is still alive.

## Host stdio and handles

A host outlives the CLI, so it must not keep a copy of the caller's standard pipes: the CLI
detaches its own three standard handles before the single spawn (Windows inherits every
inheritable handle, not only the configured stdio), gives the child its own stdio, and
captures safe pre-root host diagnostics into `<root>/cache/host-stderr.log`, truncated per
launch so repeated attempts stay bounded. Hosts started by hand still write those typed
events to stderr.

## Doctor (S01 §11.5)

Mandatory local checks read live components: root identity and held lease, credential store
readiness, storage schema/identity, real SQLite runtime version and log sink (write/read/
remove of one owned probe file plus the logger's own sink counter). `foundation.resource_manifest`
and `bridge.supervisor` are `UNSUPPORTED`, while `runtime.lmstudio`, `codec.webp_avif` and
`index.fts_vec` are `NOT_RUN`; none of them fails the foundation on its own and none is ever
reported as passed. `doctor --runtime-inventory` stays `METHOD_NOT_SUPPORTED`.

## Preview root and limits

The developer preview creates one new owned root under `.pi\preview\<canonical uuidv4>` with
the four required children, writes an ownership marker holding exactly that uuid (the marker
is created only by the preview itself) and an explicit trusted configuration, and initializes
the library through `shotd --init`. `-Action cleanup` refuses anything it did not create: the
path must be a direct child of `.pi\preview` with a canonical UUIDv4 name, a matching marker
and a canonical-UUID `library_id`, and it must contain no reparse point. The owned host must
first be stopped with a checked exit and provably gone (own root/process only, bounded wait);
the exact synthetic `ScreenMemory.Test.V1/library/<id>/owner` target is deleted by invoking
`cmdkey.exe` directly with an argument list (never through a shell, never with an unvalidated
identifier) and its absence re-verified. Only then is the owned root removed. Any failed or
uncertain step keeps the root and the credential and exits non-zero. The previous shared
`.pi\temporary\preview-build.log` is gone: every build writes into a new UUID directory, so a
preview or smoke run can never overwrite another run's evidence.
This is not a production first-run root initializer and must not be presented as one.

Known limits of this slice: whole S01 remains `IN_PROGRESS`; the full host lifecycle cases
(crash/UNC/alias/cross-logon), GUI, packaging, live runtime, codecs and index are not part of
it; and the accepted session's forced-abort path (a connection task aborted by a stop without
a drain) leaves that connection's pipe instance alive and an in-flight client request pending
until the client disconnects — the host's bounded drain keeps that path out of the normal
stop flow, and the observation is recorded as a residual risk rather than fixed here.
