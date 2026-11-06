---
applyTo: "crates/storage/src/search*,crates/storage/src/search/**,crates/storage/migrations/0008_search.sql,crates/protocol/src/search.rs,crates/application/src/search.rs,crates/cli/src/search.rs,apps/gui/src/SearchPanel*,apps/gui/src/searchLane.ts,crates/gui/src/search_lane.rs,tests/gui/**,tests/fixtures/search/**"
name: "DATA.Search"
description: "Читай при изменении effective-source FTS5/exact projection, shared SearchApi, filters, reader/session/cancellation, production gold и 10k benchmark."
---

# DATA — FTS5 / exact поиск

`sm-storage` владеет единственным writer, двумя readonly connections и bounded
reader queue32 под исходным HostLease. Search additive0008; текущие schema9/config1/application1.0,
negotiated16 capabilities после [S07 authority](SEC.agent_access.instructions.md);
SDK-протокол не меняется. 0001–0008 immutable. Preflight
root/DB остаётся прежним, upgrade/cleanup только на разрешённой синтетике, не живых
данных. Канон и источник не disposable cache; failed projection откатывает mutation.

## Источник и транзакция

Единственный resolver — `sm_application::knowledge::resolve`; source — accepted
`manual_effective_v1`. Saved capture/import/recovery материализует fallback knowledge,
source и projection **в той же** transaction. Edit/rename/delete вызывают единственный
synchronous `SearchProjectionTx`; content revision/hash, contentful FTS DELETE/INSERT,
exact keys и epoch атомарны. Нет projection triggers, model/SDK/FS IO в hook.
Reopen bounded128-row transactions материализуют legacy source и repair проверяемых
FTS/exact rows до reader/listener ready. Согласованные source/hash не переписываются;
повтор hook не повышает epoch. `CardView.projection_state=current` сверяет source и
projection; исторические immutable receipts сохраняют своё первоначальное состояние.

Семь колонок и fingerprint:
`seven_fields_6_3_1_2_2_2_1_win_nfc_unicase2.9_v1` — title, description,
category, application, window, filename, OCR; веса6,3,1,2,2,2,1. Accepted source пока
владеет только первыми тремя. Остальные колонки **пустые**, не raw context/path/history.
App/filename/OCR/taken/captured integration не реализована: не угадывать attribution,
не индексировать абсолютные origin paths. Дополнительные поля требуют source owner.

## Запрос и eligibility

Ordinary query — quoted OR <=64 maximal atoms; <=1024 scalars/4096 UTF8. Blank — browse;
nonblank без atoms — NO_LEXICAL_TERMS. FTS grammar и SQL params экранируются отдельно.
Unicode61 не morphology/translation. Exact технические atoms <=128 scalars, punctuation
`_ . : + # -`; один Rust NFC+unicase2.9 normalizer для source/query, без ё→е/stemming.
4096 distinct keys/record, deterministic priority и honest truncation flag; source/FTS
полный. Union уникален по record: all technical keys, distinct matched count, present
BM25 ASC, ingested date DESC, stable ID. Missing BM25 не равен0. Фиксированный rank
expression соответствует тем же семи весам, не wire grammar.

Все required source scopes проверяются **до LIMIT** для всей строки, не отдельных
полей. Также source hash/content revision и current original asset должны совпадать.
Поддержаны category Any/Uncategorized/AnyOf1..32 unique stable IDs, source kind и UTC
Ingested `[from,to)`. App non-Any, captured/taken/unknown/absent, hasOCR/hasAI возвращают
SEARCH_FILTER_UNSUPPORTED **до SQL admission**. Unknown filters — InvalidRequest,
не Any. Excerpt<=320 scalars, plain text с escaped controls, без HTML/raw costs/counts/
image bytes/absolute paths в search DTO. Preview/card — отдельные scoped services.

## Readers / sessions

Reader SQL budget2s, caller3s; progress handler относится только к собственному reader,
отмена/timeout никогда не interrupt writer. После каждого statement handler снимается.
Shutdown закрывает admission, drains/joins reader и file threads **до** освобождения
entry/lease и stop acknowledgement. Два bounded readers; session cache mutex сериализует
retrieval/cache mutation (нет task/thread per keystroke). Никакого snapshot между pages.

Window100..2000/default100, page1..200/default30. Cache32/4 principal, TTL5min,
IDs/revisions/flags без descriptions/images; opaque UUID cursor проверяет principal,
scope fingerprint и source epoch на каждой странице. Source change —
SEARCH_SNAPSHOT_CHANGED; после restart/TTL/eviction — SEARCH_EXPIRED. New depth = new
search ID. Bounded hostlife8192 fingerprint ledger сохраняет changed-params conflict
даже после eviction; admission после ceiling — SEARCH_BUSY, не unlimited memory.
Credential/scopes создаёт session, не wire client kind. S07 agent binding включает
connection/security revision/generation; stale binding и foreign connection cancel
отказываются даже при общем principal. Current authority и delivery publication —
[SEC.agent_access](SEC.agent_access.instructions.md).

## Клиенты и доказательства

`search.request` / `SearchApi` / ApplicationClient — общие для `gui_search` и JSON CLI.
`shot --config <absolute-config> search --text <literal> [--filters-json <SearchFilters>]
[--limit 1..200] [--candidate-depth 100..2000] [--search-id <v4>]`; `search page --cursor`
и `search cancel --search-id`. Один JSON; invalid/unsupported/no terms exit2,
stale/expired/conflict exit5, busy/cancel/timeout exit8. No model intent flags.
GUI180ms debounce + monotonic generation guard; один active и один replaceable latest
pending, без busy polling/неограниченных tasks/retry. Новый input инвалидирует старые
page/error/selection/preview. Cancel сигналит native SearchLane token вне GuiSession
mutex, queued-before-mutex caller отказывается до IO; уже отправленный shared client
request/cancel дренирует terminal response через существующий framing. Не interrupt
writer/другие principals. Текст/filters только memory; plain React text, trusted preview
assembler и scoped card.

GoldV1 immutable historical defective annotation; V2 — controlled parent source-fact
corrections, не оптимизация outputs. >=120 queries/100 cards, tuning/heldout разделены.
Legacy11/18/20 quoted-AND эксперимент не production OR quality gate. Test fixture setup
в storage не является Saved/image-file acceptance; native test отдельно импортирует
реальные synthetic images, удаляет source, проверяет default shotd/CLI/GuiSession,
canonical/export/reopen и exact credential/root cleanup при Off.

Focused Rust `sm-storage s06_` (release10k отдельно), `sm-protocol --test search`,
`sm-cli --test search_args`, affected knowledge/session; frontend lint/test/build;
Rust-generated schema + TS/Ajv shared fixtures. Benchmark actual populated production
projection/SearchApi, не standalone Python/empty table. Debug latency failure сохраняется,
release profile указывается явно. Raw логи/screenshots только fresh UUID `.pi/temporary`;
публичный краткий отчёт — `docs/progress/S06/`. Own PrintWindow != genuine input.
Required window gate отделён: `sm-gui --features owned-window-tests --test manual_knowledge`;
default offline/headless сообщает window NOT_RUN. Tracked `tests/gui/` helpers и pinned
existing Python3.14.2AMD64/Playwright1.58.0/jsonschema4.26.0 проверяются ДО fixture side
effects; нет install/browserdownload/PATH fallback. Exact chosen own endpoint provenance,
config/image/library/creation/argv/hash/non-reparse guards до CDP; failure не runtime PASS
и не permission на новый запуск/порт. Bounded test-only readiness и fail-closed guard
contract — у [APP.gui](APP.gui.instructions.md); Listen не является scenario acceptance.
Existing native/runtime failures не заменять headless или mounted evidence. Graded10-case supplement immutable после первого evaluation;
не менять policy/ожидания по outputs, old120 gold отдельно.
