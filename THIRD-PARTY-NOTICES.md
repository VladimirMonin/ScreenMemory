# Third-party notices

ScreenMemory распространяется под MIT (см. [LICENSE](LICENSE)). Ниже — сторонние
компоненты, чьи условия требуют сохранения уведомлений.

## Inspector Rust — донор кода (MIT)

Часть архитектурных решений и ограниченные фрагменты подхода к Windows-capture
взяты из открытого проекта **Inspector Rust**, фиксированная ревизия
[`fa314915273e2cb91006fa284e5dd48fa90aff3a`](https://github.com/pepperonas/inspector-rust/tree/fa314915273e2cb91006fa284e5dd48fa90aff3a).

- Copyright © 2026 Martin Pfeffer
- Лицензия: MIT
- Условия и границы заимствования: [docs/adr/001-upstream-strategy.md](docs/adr/001-upstream-strategy.md)
- Карта соответствия и хеши: [docs/progress/S01/upstream-map.md](docs/progress/S01/upstream-map.md),
  [docs/progress/S01/upstream-baseline.json](docs/progress/S01/upstream-baseline.json)

Текст лицензии донора:

```text
MIT License

Copyright (c) 2026 Martin Pfeffer

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.
```

История git донора в этот репозиторий **не входит**: она хранится только локально
как отдельная ссылка `refs/remotes/upstream/plan-baseline` и не публикуется.

## LM Studio SDK 2.0.0 (Apache-2.0)

`packages/lmstudio-bridge/vendor/sdk-2.0.0.tgz` — опубликованный SDK LM Studio,
включённый в поставку по [ADR-011](docs/adr/011-private-release-scope.md).
Лицензия: Apache License 2.0, полный текст рядом —
[`packages/lmstudio-bridge/vendor/SDK-LICENSE`](packages/lmstudio-bridge/vendor/SDK-LICENSE).
Сам LM Studio и веса моделей — внешние локальные зависимости, в репозиторий не входят.

## Rust и npm-зависимости

Точные версии зафиксированы в [Cargo.lock](Cargo.lock) и [pnpm-lock.yaml](pnpm-lock.yaml).
Уведомления и лицензии этих пакетов — в их собственных репозиториях; при
распространении собранного бинарника их тексты нужно собрать отдельно.
