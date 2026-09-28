---
applyTo: "scripts/verify.ps1,scripts/verify-contracts.ps1,scripts/verification-common.ps1,scripts/test-verify.ps1,tests/fixtures/s01/**,docs/progress/S01/t04a-verification.*,docs/progress/S01/t04b-observability.*,AGENTS.md"
name: "BUILD.Verification"
description: "Читай при изменении offline verify, фиксированного набора шагов, Windows PowerShell runner, evidence и synthetic verification fixtures."
---

# BUILD — Offline verification S01-T04a/T04b

Из корня checkout на Windows PowerShell 5.1: `powershell.exe -NoProfile
-NonInteractive -File scripts/verify.ps1`; отдельный контрактный запуск:
`powershell.exe -NoProfile -NonInteractive -File scripts/verify-contracts.ps1`.
`-OutputRoot` задаёт **новый, ещё не существующий** каталог строго под `.pi/` checkout;
по умолчанию выбирается новый UUID. Повторное использование откажет без перезаписи.
Targeted runs тоже получают отдельный уникальный output directory под `.pi/`.
Процитированные evidence artifacts неизменны: reviewer не использует writer output
path как destination и не повторяет команды, перезаписывающие старые логи.
`result.json` — UTF-8/LF/no BOM, `native-*.log[.stderr]` и ошибки MSVC — приватные
артефакты только `.pi/`; их нельзя автоматически upload, вставлять в публичный
отчёт или выводить произвольный stderr, exception, compiler payload. Сохраняй
только safe check ID, actual fixed executable/argv из того же шага исполнения
(`node @PNPM_CLI` обозначает pinned pnpm launcher, `@PRIVATE` — приватный путь),
raw exit и hashes logs. Check ID — отдельная метка, не command evidence.
`PLAN-HASH-PRE/POST` сверяют девять исходных планов с закреплёнными в
`verification-common.ps1` SHA-256; format scanner их не форматирует. Оба
позитивных runner до и после suite (даже при ошибке шага) сравнивают bytes и
набор Git tracked + nonignored untracked source, включая schemas/generated/locks;
начальное dirty дерево допустимо, mutation не чинится автоматически. Только
игнорируемые `.pi`/build/cache выходы исключены, output под `.pi` не может быть
symlink/существующим каталогом; публичный result пишется после post-check.

Prerequisites готовятся отдельно, **не** runner: проектные portable Rust 1.98.1
MSVC и Node 24.13.0, доступный pnpm package 10.32.1, установленный
`packages/protocol/node_modules` из frozen lock, VS 2022 BuildTools
`vcvars64.bat`, Python с jsonschema 4.26.0, Git для source enumeration.
Не устанавливай зависимости, не меняй registry/global PATH. Runner импортирует
VS developer environment в дочерний процесс, всем steps передаёт process-local
env, сохраняет cargo outputs под `.pi/`. Если prerequisite/version/package/script
исчез или появился новый package, запуск должен падать до ложного PASS;
изменивший workspace owner обновляет обязательный suite и каталог.

Сегодня `PASS` означает только **implemented contract subset**: Rust fmt/clippy/
test (включая S01-LOG-01/02 test-support, logger, synthetic HostLease и S01-DB-01/02 storage),
отдельные `HOST-LEASE` cargo test -p sm-isolation --test host_lease, `STORAGE-FOUNDATION` cargo test -p sm-storage (синтетические файловые DB), `TESTS-CODEC` cargo test -p sm-codec (реальный bundled libwebp exact-lossless round-trip) и `TESTS-STORAGE-IMAGES` cargo test -p sm-storage image_tests (schema 0002, канонический файл и откат), и `FRAMING` cargo test -p sm-framing (синтетические AsyncRead/AsyncWrite потоки), `SESSION-NATIVE` cargo test -p sm-session (owned native pipe + synthetic in-memory verifiers/handlers); pnpm
typecheck/lint/test/contracts:check; Python boundary/schema fixtures и source bytes.
`FRONTEND`, `BRIDGE`, полный `HOST`/`IPC` (session subset не означает готовый host), `HOST-LOG-INTEGRATION` и остальные будущие gates
остаются `NOT_IMPLEMENTED`/`NOT_RUN`;
S01 всегда `IN_PROGRESS` до отдельной приёмки владельцем. Нет live/runtime,
donor build, GUI, model download, `pnpm install` или bootstrap.
`verify-contracts.ps1` генерирует Rust/TS в новые приватные каталоги и сравнивает
checked-in bytes без overwrite-first; подробнее о DTO owner —
[BUILD.protocol_contracts](BUILD.protocol_contracts.instructions.md).

`powershell.exe -NoProfile -NonInteractive -File scripts/test-verify.ps1` проверяет
S01-VER-01 через настоящий wrapper/child exit7, raw exit, fail-fast, stdout/stderr
redaction, missing/spawn/prerequisite/timeout, paths/args с пробелами, временную
mutation только скопированного guard и tracked+untracked source bytes. Через
реальные оба позитивных runner сверяет точные argv каждого fixed cargo/node/pnpm/
Python/PowerShell шага; isolated copied host проверяет expected RED для порчи и
отсутствия pinned плана, child source modify/create/delete/failed-exit7 с обязательным post guard, без затрагивания
исходных планов и настоящих source guards. В
`tests/fixtures/s01/verification-manifest.json` — checksum синтетического child;
не подставляй пользовательские пути/fixtures. Format scan читает кандидаты через
`git ls-files --cached --others --exclude-standard -z`, UTF-8 strict, BOM/CRLF/
trailing whitespace; девять immutable исходных планов исключены из форматтера,
их SHA-256 проверяются отдельно. Negative test сам удаляет только свой fixture.
Изменения checked-in manifests, schemas, README, lockfiles и планов от runner
недопустимы: сохраняй pre/post hashes и проверяй diff/status. Сохранение code
commit/принятие stage — у интегратора, не у runner.
