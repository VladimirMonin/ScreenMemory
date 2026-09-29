---
applyTo: "crates/capture/**,crates/application/src/capture.rs,crates/host/src/capture.rs,crates/host/src/bootstrap.rs"
name: "CAPTURE.Native"
description: "Читай при изменении захвата экрана, region picker, overlay, hotkey, host capture command."
---

# CAPTURE — захват экрана

S02 §3–§8, §11–§13. Четыре режима: Region (overlay + crop), ActiveWindow, CurrentMonitor, AllMonitors. GDI DIB top-down без DDB-fallback. freeze timestamp ДО encode — `frozen_at_ms` передаётся через `CaptureResult` в `canonical_utc_millis_from_unix_ms`. Отмена queued сессии через `Shared.cancelled` — `begin_session` проверяет перед freeze.

Порядок: freeze → overlay/pick → crop → staging → verified encode → DB commit. `CapturedDib` — RAII handles/DC/bitmaps. GetDIBits deselect ДО release DC. GetMessageW три исхода. Hotkey loop — bounded IPC callback (capacity 2). PIP — `shot capture --mode region`, GUI кнопка — аргумент-free команда.

T09e (только command): после replay/probe/admission отдельная host-owned Tokio-задача удерживает `InFlightGuard`, save slot, native result и последующий verified commit; отмена IPC waiter сигналит native cancel, но не отменяет уже принятый кадр. При штатном `serve → run_draining → finish` с живым runtime shutdown закрывает admission и дожидается зарегистрированных command-задач **до** `storage.shutdown`; за пределом grace сохраняет lease до их завершения и выдаёт non-success exit. Это не распространяется на abort/drop `serve` или остановку runtime, где сохранность volatile crop не доказана. T09f заменяет abort hotkey consumer на общий drain; T09g подключает host stage. `durable_input`/recovery остаются неподтверждёнными — не выдавать эти срезы за их PASS.

T09f: `HotkeySink::try_reserve` неблокирующе до `run_session`; `HotkeyReservation` RAII держит общие с command save slot, регистрацию gate и `OwnedPermit` bounded hotkey queue. Отказ — без freeze; `deliver` переносит исход в queue, consumer выполняет verified commit без повторного слота. При остановке gate закрывается до receiver.close, выданные permits дочитываются и consumer joined до storage.shutdown; после grace lease остаётся до завершения, exit non-success. Bind failure после старта consumer закрывает/drain/join и shutdown storage. Это count/queue bound, **не** доказательство 1 GiB/RSS; отдельный byte-budget и аварийный runtime-abort остаются открытыми ограничениями полной S02.

T09g: command и hotkey после принятия одного DIB передают owned нормализованный RGBA и доверенные library/principal/operation/record/asset/frame IDs, mode/context/freeze time/encoder fingerprint в `StorageWorker::stage_capture_owned` **до** encode/DB. `Staged` сверяется с удержанным результатом; canonical проверяется на соответствие тому же DIB и профилю; `Saved` выдаётся только после прежнего verified commit. При временно полном файловом канале owned input остаётся в ожидающей host-задаче (capacity канала не растёт); при ошибке уже созданную incomplete stage не удалять, тот же operation ID fail-closed до recovery. Опубликованный manifest не означает power-loss durability/`durable_input`; startup reconciliation, scoped status/retry, process-kill и byte-budget 1 GiB ещё NOT_RUN.

Тесты: `cargo test --locked --offline -p sm-capture -- --test-threads=1` (9 native). `s02_nat_01..09`. Verify: `CAPTURE-NATIVE` step. Глобальные hotkey и tray — NOT_RUN без разрешённого тестового окружения.