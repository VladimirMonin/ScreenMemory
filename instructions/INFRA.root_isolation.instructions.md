---
applyTo: "crates/isolation/**,Cargo.toml,Cargo.lock,tests/contracts/check_boundaries.py,docs/progress/S01/t02b-isolation.*,docs/adr/003-root-isolation.md"
name: "INFRA.RootIsolation"
description: "Читай при изменении namespace, Windows root identity, trusted installed/test root, donor overlap и native pre-host isolation tests."
---

# INFRA — Root isolation до host

`sm-isolation` — единственный текущий владелец identity продукта/окружения и read-only Windows resolver. Installed configuration явно содержит существующие четыре каталога `ScreenMemory[.Dev|.Test]/{data,config,cache,webview}` и явно известные forbidden donor roots; test требует explicit root с четырьмя заранее созданными дочерними каталогами. Ни CWD, ни home, ни PATH не используются как fallback. Не считывать реальные settings/history/credentials Inspector для поиска donor roots. Если известный forbidden root не разрешается, отказ, не исключение из проверки. Никаких изменений существующих папок resolver не делает.

До открытия БД понадобится отдельный HostLease/ownership/migration preflight. Этот resolver не выдаёт разрешения на SQLite writes, не создаёт каталогов и не очищает непустые данные. Канон и принятый staging — protected source of truth; test roots — только `.pi/temporary` со строго синтетическими sentinel bytes, cleanup только собственной fixture после её проверок. Не распространять test cleanup на живой каталог, не обещать физическое удаление на SSD. Backup/integrity/hash/restore policy для persistent data определит будущий storage owner до первой записи; отсутствие проверенного backup означает отказ в потенциально разрушительной миграции. Никаких автозапусков updater, donor shell или чужих сервисов.

`windows.rs` изолирует Win32 SDK ABI: local fixed NTFS, каждый ancestor и leaf открываются с `OPEN_REPARSE_POINT|BACKUP_SEMANTICS` и проверяются по handle, root handle удерживается. Обратная последовательная проверка при hostile concurrent filesystem mutation и lock/TOCTOU hardening принадлежат T05; пока host писать не вправе. Directory identity и полный путь внутренние, не DTO/status/обычные логи. Diagnostics — только typed `SafeEvent` и bounded safe code. Junction и symlink различать: непривилегированный junction proof обязателен, symlink может дать точный NOT_RUN с raw OS code без elevation/Developer Mode/ACL changes. Проверки: `scripts/verify.ps1`, `scripts/verify-contracts.ps1` после обновления workspace boundary, raw exits и source/plan integrity сохраняются. Политика/ограничения: [ADR-003](../docs/adr/003-root-isolation.md).
