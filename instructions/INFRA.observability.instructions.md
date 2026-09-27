---
applyTo: "crates/application/src/observability.rs,crates/observability/**,docs/progress/S01/t04b-observability.*,scripts/verify.ps1"
name: "INFRA.Observability"
description: "Читай при изменении SafeEvent, bounded logger, test clock, log sinks, ротации или проверок S01-LOG-01/02."
---

# INFRA — Безопасная диагностика

`sm-application::observability` владеет закрытым словарём `SafeEvent`/`SafeEventSink`:
только enum, числа и opaque UUID bytes, без свободного текста, сериализации
request/SDK exception или `Debug` произвольных payload. Calendar UTC epoch milliseconds
и монотонное время для rate limit/duration — независимые источники. Не добавляй
path, prompt, query, title, pixels, GPS, headers или credentials в обычные события.
Новый вариант enum требует synthetic canary/negative coverage; terminal записывается
только реальным owner после действительного outcome, не при восстановлении логгера.

`sm-observability` — инфраструктурный адаптер. Публичные production sinks принимают
только `&SafeEvent` через `LogSink::write_event`; сериализация, raw JSONL writer и
проверка 512 bytes остаются внутри crate и проверяются также при прямом вызове
`ConsoleSink`/`FileSink` без очереди. Не открывай `&str`/`Write` API для событий.
Внешний consumer может реализовать typed `LogSink`, но ответственность за его
собственную безопасность не заменяет проверку встроенных sinks. Явный уже созданный owned log root,
без default home, startup side effect и symlink/junction cleanup. Максимум 8192
queued событий, 512 bytes сериализованного event, WARN/ERROR вытесняют обычные
события в той же очереди. `emit` — try-lock/try-queue; `false` и dropped counter
не гарантируют delivery. `shutdown(deadline)` сообщает timeout отдельно от
flush/sink failure; при timeout сохраняй owner, дай sink завершиться и повтори
shutdown, не объявляй flushed. Для полного process exit зависший сторонний
sink может оставить worker; интеграционный owner обязан держать lifetime bounded.
Файлы ротации — только `operational-<UTCms>-<sequence>.jsonl` в явно заданном root;
лимиты одновременно 5 × 10 MiB и age 14 days, тестовые меньшие лимиты допустимы.

`cargo test --workspace --locked` в `scripts/verify.ps1` запускает typed logger,
allowlist/canary и S01-LOG-02 вместе с контрактами. S01-LOG-01 подтверждён
лишь для typed boundary/собственных file/console/stderr sinks: SDK exception,
header, prompt, path и model response пока не поступают через реальный producer;
сквозная часть S01-LOG-01 отложена до host/bridge. Test-only ignored mutation
приватного serializer→writer edge запускается отдельно и должна падать по утечке
canary в реальных bytes, не по искусственному panic. `scripts/verify-contracts.ps1` проверяет workspace/dependency
boundary. Synthetic evidence держи под `.pi/`, не публикуй compiler stderr и
fixture canaries. Пока host/bridge/GUI отсутствуют, реальная сквозная integration,
doctor и crash lifecycle — `NOT_IMPLEMENTED`, не считать их проверенными.
