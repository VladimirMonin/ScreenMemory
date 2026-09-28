---
applyTo: "crates/capture/**,crates/application/src/capture.rs,crates/host/src/capture.rs"
name: "CAPTURE.Native"
description: "Читай при изменении захвата экрана, region picker, overlay, hotkey, host capture command."
---

# CAPTURE — захват экрана

S02 §3–§8, §11–§13. Четыре режима: Region (overlay + crop), ActiveWindow, CurrentMonitor, AllMonitors. GDI DIB top-down без DDB-fallback. freeze timestamp ДО encode — `frozen_at_ms` передаётся через `CaptureResult` в `canonical_utc_millis_from_unix_ms`. Отмена queued сессии через `Shared.cancelled` — `begin_session` проверяет перед freeze.

Порядок: freeze → overlay/pick → crop → encoded → commit. `CapturedDib` — RAII handles/DC/bitmaps. GetDIBits deselect ДО release DC. GetMessageW три исхода. Hotkey loop — bounded IPC callback (capacity 2). PIP — `shot capture --mode region`, GUI кнопка — аргумент-free команда.

Тесты: `cargo test --locked --offline -p sm-capture -- --test-threads=1` (9 native). `s02_nat_01..09`. Verify: `CAPTURE-NATIVE` step. Глобальные hotkey и tray — NOT_RUN без разрешённого тестового окружения.