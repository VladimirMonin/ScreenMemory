---
applyTo: "crates/capture/**,crates/application/src/capture.rs,crates/host/src/capture.rs,crates/host/src/bootstrap.rs"
name: "CAPTURE.Native"
description: "Читай при изменении захвата экрана, region picker, overlay, hotkey, host capture command."
---

# CAPTURE — захват экрана

S02 §3–§8, §11–§13. Четыре режима: Region (overlay + crop), ActiveWindow, CurrentMonitor, AllMonitors. GDI DIB top-down без DDB-fallback. freeze timestamp ДО encode — `frozen_at_ms` передаётся через `CaptureResult` в `canonical_utc_millis_from_unix_ms`. Отмена queued сессии через `Shared.cancelled` — `begin_session` проверяет перед freeze.

Порядок: freeze → overlay/pick → crop → encoded → commit. `CapturedDib` — RAII handles/DC/bitmaps. GetDIBits deselect ДО release DC. GetMessageW три исхода. Hotkey loop — bounded IPC callback (capacity 2). PIP — `shot capture --mode region`, GUI кнопка — аргумент-free команда.

T09e (только command): после replay/probe/admission отдельная host-owned Tokio-задача удерживает `InFlightGuard`, save slot, native result и последующий verified commit; отмена IPC waiter сигналит native cancel, но не отменяет уже принятый кадр. При штатном `serve → run_draining → finish` с живым runtime shutdown закрывает admission и дожидается зарегистрированных command-задач **до** `storage.shutdown`; за пределом grace сохраняет lease до их завершения и выдаёт non-success exit. Это не распространяется на abort/drop `serve` или остановку runtime, где сохранность volatile crop не доказана. T09f заменяет abort hotkey consumer на общий drain; host stage/`durable_input`/recovery не подключены — не выдавать этот срез за их PASS.

T09f: `HotkeySink::try_reserve` неблокирующе до `run_session`; `HotkeyReservation` RAII держит общие с command save slot, регистрацию gate и `OwnedPermit` bounded hotkey queue. Отказ — без freeze; `deliver` переносит исход в queue, consumer выполняет verified commit без повторного слота. При остановке gate закрывается до receiver.close, выданные permits дочитываются и consumer joined до storage.shutdown; после grace lease остаётся до завершения, exit non-success. Bind failure после старта consumer закрывает/drain/join и shutdown storage. Это count/queue bound, **не** доказательство 1 GiB/RSS; отдельный byte-budget и аварийный runtime-abort остаются открытыми ограничениями полной S02.

Тесты: `cargo test --locked --offline -p sm-capture -- --test-threads=1` (9 native). `s02_nat_01..09`. Verify: `CAPTURE-NATIVE` step. Глобальные hotkey и tray — NOT_RUN без разрешённого тестового окружения.