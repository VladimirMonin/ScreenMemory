---
applyTo: "crates/codec/**"
name: "CODEC.WebP"
description: "Читай при изменении bundled WebP, exact lossless, export quality90, bounds и capture/recovery API."
---

# CODEC — WebP exact lossless

Фактический manifest/lock: static bundled `libwebp-sys =0.14.4`, default-features=false,
features=std. `0.37.0` в прежней инструкции было ошибкой, не shipped версией.
Native revision читается из linked libwebp в `encoder_fingerprint()`.

Capture/recovery сохраняют `webp-lossless-v1`: lossless=1, exact=1, method4,
quality100, thread_level0; RGBA/BGR/gray-alpha и RGB при alpha0 проверяются побайтно.
`encode_lossy90_export` — отдельный S03 export-only путь: lossless0, quality90,
alpha_quality100, method4/thread0; не меняет canonical API/fingerprint.
Bounds/typed malformed отказ принадлежат `CodecLimits`; S03 добавляет более узкий
96MP/working-budget guard **до** decode недоверенного входа.

Legacy `sm-codec::{capability,encode_canonical,decode_canonical}` остаётся WebP-only:
его AVIF ответ честно NotImplemented. Рабочий S03 AVIF принадлежит отдельному
[IMAGE.pipeline](IMAGE.pipeline.instructions.md), а не фиктивному capability flag.
`sm-codec` не имеет FS/SQL/сети/логов. Native untrusted input — только private worker.

Проверки: `cargo test --offline --locked -p sm-codec`, `TESTS-CODEC` в verify;
S03 export и совместимость exact проверяются также `sm-image`.
