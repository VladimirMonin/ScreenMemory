---
applyTo: "crates/capture/**,crates/application/src/capture.rs,crates/host/src/capture.rs"
name: "CAPTURE.Native"
description: "Читай при изменении захвата экрана, region picker, overlay, hotkey, host capture command."
---

# CAPTURE — захват экрана

S02 §3–§8, §11–§13. Четыре режима: Region (overlay + crop), ActiveWindow, CurrentMonitor, AllMonitors. GDI DIB top-down без DDB-fallback. freeze timestamp ДО encode — `frozen_at_ms` передаётся через `CaptureResult` в `canonical_utc_millis_from_unix_ms`. Отмена queued сессии через `Shared.cancelled` — `begin_session` проверяет перед freeze.

Порядок: freeze → overlay/pick → crop → encoded → commit. `CapturedDib` — RAII handles/DC/bitmaps. GetDIBits deselect ДО release DC. GetMessageW три исхода. Hotkey loop — bounded IPC callback (capacity 2). PIP — `shot capture --mode region`, GUI кнопка — аргумент-free команда.

T09e (только command): после replay/probe/admission отдельная host-owned Tokio-задача удерживает `InFlightGuard`, save slot, native result и последующий verified commit; отмена IPC waiter сигналит native cancel, но не отменяет уже принятый кадр. При штатном `serve → run_draining → finish` с живым runtime shutdown закрывает admission и дожидается зарегистрированных command-задач **до** `storage.shutdown`; за пределом grace сохраняет lease до их завершения и выдаёт non-success exit. Это не распространяется на abort/drop `serve` или остановку runtime, где сохранность volatile crop не доказана. Hotkey consumer всё ещё abort при shutdown, а host stage/`durable_input`/recovery не подключены — не выдавать этот срез за их PASS.

Тесты: `cargo test --locked --offline -p sm-capture -- --test-threads=1` (9 native). `s02_nat_01..09`. Verify: `CAPTURE-NATIVE` step. Глобальные hotkey и tray — NOT_RUN без разрешённого тестового окружения.