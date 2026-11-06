---
applyTo: "crates/storage/src/knowledge/**,crates/storage/src/knowledge.rs,crates/storage/migrations/0007_knowledge.sql,crates/protocol/src/knowledge.rs,crates/application/src/knowledge.rs,crates/cli/src/knowledge.rs,crates/gui/tests/manual_knowledge.rs,apps/gui/src/KnowledgePanel*"
name: "DATA.Knowledge"
description: "Читай при изменении manual карточки, Keep/Inherit/Set, категорий, replay/revision guards, sensitivity и versioned effective source/transaction hook S06."
---

# DATA — ручная карточка и источник поиска

`sm-storage` остаётся единственным SQLite writer/owner lease. Additive `0007_knowledge`
не меняет SQL/checksums0001–0006, records/assets/origins/source snapshots или канон.
Перед open/migration прежний root/DB preflight; живые библиотеки и destructive upgrade
без принятого backup authority запрещены. Проверки — собственные synthetic UUID roots.
Это manual-only S05, не OCR, AiRun/history/publisher или preferences/privacy enrollment.
FTS5 integration — [DATA.search](DATA.search.instructions.md).

## Порт и полномочия

Application1.0 включает `knowledge.request` и S06 `search.request` (native negotiation15 capabilities);
SDK sm-sdk1.1 и legacy probe bridge1.0 независимы и не меняются.
После S07 authority negotiation16/schema9; metadata-only agent global maintenance
Categories явно отказывает (Owner неизменен), current authority/delivery —
[SEC.agent_access](SEC.agent_access.instructions.md). Закрытые typed actions:
`read`, `edit`, `categories`, `category`. `KnowledgeApi` получает настоящий RequestContext;
ReadMetadata/WriteMetadata/ManageCategories проверяются session и SQLite worker.
TestReadOnly не получает их. Wire не принимает principal, scopes, путь, lease,
AI result/approval или backend authority. Ошибки закрытые, без пользовательского текста.

UUID — canonical RFC4122 v4. Revisions — decimal strings0…i64::MAX, не JSON Numbers.
Title — непустое непробельное≤160 Unicode scalars; description≤8000, намеренная пустая
строка допустима. Для title/name «пробельное» — явный Unicode White_Space набор:
U+0009–000D,0020,0085,00A0,1680,2000–200A,2028,2029,202F,205F,3000 (Rust trim).
Не использовать платформозависимый regex `\s`: U+FEFF и U+001C–001F не входят в набор;
NUL запрещён отдельно. Rust/Ajv/Python проверяют общий positive/negative fixture. `FieldPatch` отсутствует/Keep — оставить override; Inherit — снять
ровно этот override; Set category null — намеренно без категории. Closed decoder
отказывает extra fields даже для Keep/Inherit. Нет общего edited-флага.

`sm_application::knowledge::resolve` — единственный pure effective resolver:
явный user → applicable accepted AI → техническое имя/пустое/null. Proposal — только
pure private/native boundary для будущего publisher, не RPC и не доказательство
live AI. До предложения будущий owner обязан проверить job/attempt/lease/host,
authorization/privacy, asset+SHA и intent fences. Analyze/OCR content_revision=None;
ручной record_revision не универсальный hard input fingerprint. Publisher пока отсутствует.

## Транзакции, источник и replay

Edit и category mutation: stable operation UUID + principal/fingerprint → atomic
expected record/category/catalog revisions → overrides/category/history → effective/source
→ synchronous SearchProjectionTx/debt → immutable committed receipt → COMMIT.
Идентичный потерянный ответ возвращает прежний receipt; изменённые params/principal —
IDEMPOTENCY_CONFLICT. Нет last-write-wins или автоматического retry с новой revision.
Category history и receipts имеют immutable UPDATE/DELETE triggers.

`manual_effective_v1` snapshot содержит effective text, category ID/label, asset identity
и known required read scopes; не context paths/GPS/serial/window title. Read без knowledge
row даёт side-effect-free technical fallback. S06 initial Saved/backfill материализует
source/projection атомарно; read сообщает Current только при согласованных source/hash/projection.
Content revision/hash меняются только при изменении source, не provenance/receipt bookkeeping.
Known sensitive scopes сохраняются при ручной правке/Inherit/category maintenance, не
понижаются. Worker проверяет их и при replay; ListRecords вместо restricted derived title
выдаёт безопасное техническое имя. Никакого declassification-флага в edit.

SearchProjectionTx — только синхронный storage-owned hook той же transaction, без SQL
escape в application, IO/model/async wait или второй connection. Ошибка hook откатывает
всю mutation, включая receipt и все category projections. S06 adapter синхронно обновляет FTS5/exact/epoch и снимает debt; read/state/DTO
различают Current и historical HandlerUnavailable. Ошибка проекции — rollback всей mutation.
Нет embeddings/vector/sqlite-vec или model call от edit/read/категорий.

## Каталог и границы

User-owned IDs; trim + Windows NFC + full pinned unicase2.9 folding, key version
`win_nfc_unicase2.9_v1`, не SQLite ASCII NOCASE. Name≤80 scalars, description≤400.
Active/archived имена уникальны по key. Active ceiling128; bounded catalog≤512 IDs,
включая tombstones; превышение явно refused, не обрезанная выдача.
Create — INSERT-only, даже UUID collision не усыновляет историю. Rename сохраняет ID.
Archive запрещает новые назначения, но оставляет существующие ссылки. Restore разрешён
только archived; deleted UUID никогда не восстанавливается/переиспользуется. Delete
проверяет показанное expected_assigned_records и делает manual Set(None), не Inherit.
Rename/delete и affected effective/source revisions — один COMMIT. Maintenance>4096
assigned records явно refused; 10/50k perf/barrier acceptance не заявляется.

## Клиенты и проверки

`gui_knowledge`/GuiSession и `shot knowledge` используют один ApplicationClient, без
второго resolver/SQLite/model в WebView. GUI хранит черновик только в памяти. Conflict
показывает текущую карточку рядом; reload/discard и retry с новой revision/operation —
явные действия. Не сохранять новый text-bearing localStorage. Mount — readonly only.

CLI: `knowledge read --record-id`; `knowledge categories`; `knowledge category
--request-json <typed CategoryMutation>`; `knowledge edit --record-id --operation-id
--expected-revision --catalog-revision`, независимые `--title`, `--description`,
`--category-id --category-revision`, `--clear-category true`, `--inherit-title true`,
`--inherit-description true`, `--inherit-category true`. Конфликтующие patches отказываются
до IPC. Всегда один JSON; revision/replay/name conflict exit5, остальные bounded refusals
по shared CLI registry. Config schema1 unchanged, storage schema8 exact после additive0008.

Focused tests — `sm-storage knowledge`, `sm-protocol --test knowledge`, `sm-cli --test
knowledge_args`, `sm-session s05_native_knowledge`, frontend KnowledgePanel tests.
`sm-gui --test manual_knowledge` запускает default binaries, собственные child handles,
реальные Saved/CLI/GuiSession/replay/conflict/reopen/export при Off и exact owner credential
cleanup/absence. Default binaries сначала собираются offline/locked без fault features.
Private runtime evidence — новые UUID `.pi/temporary` directories; не перезаписывать
старые failures/logs/screenshots. Обычный rendered proof — только own HWND PrintWindow;
genuine form input остаётся NOT_PROVEN без разрешённого clean guest.
