---
applyTo: "crates/codec/**"
name: "CODEC.WebP"
description: "Читай при изменении WebP codec, bounds, limits."
---

# CODEC — WebP exact lossless

Bundled libwebp-sys (static, pinned `0.37.0`). Только `WebP exact lossless`. RGBA/BGR/grayscale round-trip, alpha0 RGB preservation. Bounds: geometry × budget (16384 сторона, 16383² пикселя). Empty/truncated/malformed → typed отказ, не panic/decoded garbage.

`AVIF` — `NOT_IMPLEMENTED` (typed `CodecCapability::Avif`). `WAIF_CODEC` — типизированные ошибки.

Тесты: `s02_codec_01..09`. Verify: `TESTS-CODEC` step.