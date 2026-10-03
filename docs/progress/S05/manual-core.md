# S05 manual-core — scoped приёмка

Статус parent: **ACCEPT_CODE_SCOPED** после CHECKED writer, fresh independent review
и закрытия двух P2. **Не whole S05 DONE.** Baseline
`d2eb666a1a257d5dedca99366a104c2b76b52390`. Writer/reviewer не выполняли staging/commit;
локальная фиксация принятого среза — parent. Планы и migrations0001–0006 не изменены.

## Что работает

Настоящий import→Saved при Off → typed card, собственные категории и независимые
Keep/Inherit/Set title/description/category через один native KnowledgeApi для JSON CLI
и GUI. Пустое описание, Set(null) и Inherit различаются. Stable operation replay,
principal/fingerprint-bound conflict, expected record/category/catalog revisions;
нет last-write-wins. GUI сохраняет in-memory draft на конфликте, показывает current
snapshot и предлагает явный discard/retry. Нового text-bearing localStorage нет.

Sole SQLite worker, additive0007/schema7/config1/application1.0+14 capabilities.
Windows NFC/full casefold categories,128 active/512 total bound, immutable history/receipts;
archive сохраняет связи, delete делает counted Set(None), deleted UUID не используется
повторно. Rename/delete + все affected effective/source rows + receipt атомарны.

Единственный pure effective resolver, `manual_effective_v1`, content revision/hash только
при изменении source; sensitivity/read scopes не снимаются ручной правкой. Transaction
SearchProjectionTx/debt seam отдаёт **handler_unavailable**, не Indexed/Current. Read,
edit и category mutation не обращаются к SDK/model. Analyze/OCR content_revision=None,
job/attempt/lease/privacy/asset/intent fences не ослаблены. Late-proposal test — pure
private boundary, **не** runtime AI proof.

Owner rules: [DATA.knowledge](../../../instructions/DATA.knowledge.instructions.md).

## Реальные gates

Evidence base **E**: `.pi/temporary/81ac24c5-9202-4215-98e3-ab9106483e44/`.
Логи immutable, ранние failures сохранены. Rust **R**: `cmd /d /c .pi\t09b-run.cmd`;
в таблице аргументы после R, Rust1.98.1/pinned MSVC, offline+locked. Node **N**:
private Node24.13.0 + существующий pnpm10.32.1, без install/download.

| Проверка/реальная команда | Raw exit | Evidence |
|---|---:|---|
| R `test --offline --locked -p sm-storage --lib -- --test-threads=1` | 0 | E`final-affected-1.log`:137 PASS; один child-helper ignored по design, запускается owning parent |
| R `test --offline --locked -p sm-application --lib` | 0 | E`final-affected-2.log`:8 PASS, включая late pure proposal |
| R `test --offline --locked -p sm-protocol` | 0 | E`final-affected-3.log`:11 PASS |
| R `test --offline --locked -p sm-cli -- --test-threads=1` | 0 | E`final-affected-4.log`:12 PASS |
| R `test --offline --locked -p sm-host --test composition -- --test-threads=1` | 0 | E`final-affected-5.log`:5 PASS |
| R `test --offline --locked -p sm-gui -- --test-threads=1 --nocapture` | 0 | E`final-affected-6.log`:17 PASS, включая actual default native integration |
| R `test --offline --locked -p sm-session -- --test-threads=1` | 0 | E`final-rust-3.log`, native/wire/scopes/parity |
| R `clippy --offline --locked -p sm-protocol -p sm-application -p sm-storage -p sm-session -p sm-host -p sm-cli -p sm-gui --all-targets -- -D warnings` | 0 | E`final-affected-7.log` |
| R `fmt -p sm-protocol -p sm-application -p sm-storage -p sm-session -p sm-host -p sm-cli -p sm-gui -- --check` | 0 | E`final-affected-8.log` |
| R `build --offline --locked -p sm-host -p sm-cli -p sm-gui -p sm-image-worker --no-default-features` | 0 | E`final-rust2-3.log`, normal production binaries, no fault features |
| R `run --offline --locked -p sm-protocol --example generate_schemas -- --check schemas` | 0 | E`final-rust2-4.log`; первоначальный absent generation: E`schema-gen1/`, `schema1.log`0 |
| N `--filter @screenmemory/gui typecheck`, `lint`, `test`, `build` | каждый0 | E`final-frontend-0..3.log`:22 renderer tests PASS; parity/boundaries входят в lint |
| N `--filter @screenmemory/protocol test`; `contracts:check` | каждый0 | E`final-frontend-4..5.log`; первоначальная absent TS generation E`ts-gen1/`, `tsgen1.log`0 |
| `python tests/contracts/check_schemas.py` | 0 | E`python-final.log`: shared Rust/Ajv/Python positive+negative knowledge fixtures |
| `python E/finish-runtime.py` | 0 | E`b4ab2160-bb20-4602-a438-4e30edf9d282/result.json`: native rename/replay/reopen state/export/preservation/render/guarded cleanup |

Новые регрессии (все реально PASS):

- `manual_knowledge_shared_rust_ajv_python_positive_negative_parity`;
  `manual_text_scalar_bounds_and_signed_sqlite_decimal_revisions`.
- `late_applicable_proposal_merges_current_overrides_not_record_revision`.
- `manual_saved_replay_conflict_keep_inherit_empty_unicode_reopen_and_canonical_preservation`.
- `categories_nfc_casefold_atomic_rename_delete_archive_restore_and_revision_checks`.
- `projection_failure_rolls_back_edit_receipt_category_and_all_affected_sources`.
- `additive_v6_saved_upgrade_rollback_reopen_preserves_metadata_and_canonical`.
- `known_sensitive_source_scopes_survive_manual_edits_and_are_not_listed_or_replayed_to_denied_reader`.
- `concurrent_clients_expected_revision_has_exactly_one_winner_without_last_write_wins`:
  две независимые client runtime threads + barrier, ровно один winner/один RevisionConflict.
- `active_category_ceiling_refuses_create_and_restore_without_partial_catalog_or_receipt`.
- `manual_cli_closed_patch_vocabulary_revisions_unicode_and_no_authority`.
- `s05_native_knowledge_closed_scope_and_truthful_uncomposed_port`.
- `s05_actual_default_shotd_cli_gui_manual_replay_conflict_reopen_export_ai_off`:
  настоящие default binaries, own child handle/root/library, source удалён после Saved;
  CLI+GuiSession equality, conflict exit5, Off/bridge stopped, stop/wait raw0, reopen,
  export/canonical unchanged, exact owner credential cleanup/absence. Не MockSession proof.
- KnowledgePanel3 tests: StrictMode SSR без IPC/model (не доказательство browser effects);
  independent empty/null patches; stable retry и conflict draft без overwrite. В live
  own-HWND показана загрузка native карточки без autoAnalyze.

Обновлены exact schema7/init-object oracles в storage image/processing/migration tests,
CLI `init`, GUI schema checks и native capabilities14 session/host composition tests;
их affected packages выше PASS. Standalone S01 smoke script изменён на exact7, но полный
foundation smoke **NOT_RUN**: actual init/status7 уже проверены native integration.

## Saved-v6 upgrade и визуальный proof

До rebuild сохранены прежние default binaries. Через собственный preview wrapper
`68dfe8a5-35e6-4bcd-b51e-0fe1399ea842` выполнен настоящий v6 import odd-alpha synthetic
PNG, original input удалён, host stopped0. Baseline snapshot:
E`2d7ec2ec-dd6b-45e2-a612-7e00db25aeb7/snapshot.json` (schema6, rows4 tables,
metadata/canonical SHA). Затем actual v7 open → edit/category/replay/conflict → stop/reopen
→ export. Каждый native raw exit0 либо ожидаемый conflict5; прежние records/assets/
origin_occurrences/asset_processing rows, metadata и canonical SHA **byte-exact сохранены**.
Rollback/new DB layer дополнительно проверены repository regression выше.

Actual runtime archive E`abc1caf0-07d7-4e29-af9b-6e2e81ae014b/result.json` валидирован
follow-up raw0. Первоначальный helper завершил ВСЕ assertions и записал result, но его
терминальный summary `→` упал на cp1251 (wrapper raw1); это не скрыто как aggregate PASS.
Follow-up ASCII summary raw0, реальное native rename/replay/export/render и отдельная
повторная проверка baseline-preservation; exact cleanup/credential absence raw0.

[Rendered PNG](../../../.pi/preview-artifacts/aa1caddc-e31e-4dab-9b4a-59a14941de52/owned-window.png),
[PrintWindow descriptor](../../../.pi/preview-artifacts/aa1caddc-e31e-4dab-9b4a-59a14941de52/result.json): own HWND,
parent сохранил raw proof byte-exact в ignored `.pi` перед commit по WORKFLOW.release;
SHA PNG `dd222f2ec0d0ec0341b4b6fb96ea09b1dc2509324830f78b6db4bc94793ba232` не изменён.
physical1462×1136, DPI144, desktop_capture=false. Агент осмотрел: RU/EN title/description,
категория, три independent Keep modes, native record/content revisions3, честный
«обработчик недоступен», кнопка сохранения и собственный category catalog видны/читаемы.
Старый первый скрин показывал settings выше карточки; editor перемещён перед settings,
frontend/rebuild gates повторены. Никаких screenshots личного desktop/GetDC(NULL).
**Genuine form input NOT_PROVEN** без clean guest; SSR/pure renderer и native GuiSession
не выдаются за ручной ввод. Собственные GUI/host handles закрыты/WaitForExit0; fixture/root
и exact Test credential удалены штатным guarded cleanup, отсутствие проверено.

## Исторические failures и исправленные дефекты; scope остаётся открытым

Early raw101 gates сохранены: missing cached UUID dependency; stale exact schema/table
assertions; closed Keep decoder discrepancy; clippy initialization warning; нормализация
Windows input path в native harness; standalone storage race test не имел Tokio macros —
заменён реальными barrier threads без новой dependency. Поздние affected gates0.

Отдельно сохранён **S05 historical native fail-fast raw3221226505 = 0xC0000409 /
STATUS_STACK_BUFFER_OVERRUN**: E`affected1-0.log:132–137`, executable
`sm_storage-9fada012410584a7.exe --test-threads=1`; последняя test-label —
`tests::s01_db_02_fault_rollback_and_interrupted_empty_table`. Root cause
**UNKNOWN/NOT_DIAGNOSED**: backtrace/causal diagnosis в логе отсутствуют, имя NTSTATUS
не доказывает stack-buffer defect. Поздние affected GREEN не доказывают устранение
причины crash. Это отдельный S05 receipt, не исторический S03 crash; широкий повторный
runtime аудит ради этой формулировки не выполнялся.
Два ошибочных вызова несуществующих root scripts `fixtures:check`/`parity:check` raw1
(E`final-frontend-6..7.log`), НЕ gates PASS: фактические manifests проверены, существующие
GUI lint parity/boundaries и Python fixtures действительно исполнены0.

## Fresh review и parent P2 correction

Fresh readonly native Sol6.1 review `a74f1410-10ef-4cd3-9056-e3cabcbb5590`:
**fix**, два P2; source/docs/Git не менял. Independent actual native scenario1/1 и
storage knowledge7/7 — PASS/raw0 в
`.pi/temporary/352a690d-1511-4502-9740-b0e60043edd7/`. Canonical/metadata preservation,
AI Off/bridge stopped, replay/conflict/reopen/export и exact fixture cleanup проверены.
Whole S05, genuine form input и OCR/vision reviewer не принимал.

- Whitespace P2: schema `\s` расходился между Rust/Ajv/Python для U+0085/U+FEFF.
  Parent сохранил Rust trim semantics и задал явный Unicode White_Space regex для
  CardTitle/CategoryName. Общие fixtures теперь24, включая оба codepoints, полный
  White_Space набор и Python-specific U+001C–001F; добавлен native scalar regression.
  RED Ajv1/Python1: `.pi/temporary/4bf76ec3-b0f5-41db-99fb-7801f295d113/`.
  Rust в том batch получил ошибочный TAB в runner argv/raw1 **до тестов**, не TDD-red;
  исправлен только argv, тот же runner/Rust baseline2/2 PASS/raw0:
  `.pi/temporary/e035279f-c2df-4a7e-a750-bd123875896c/`. Runtime isolation не менялась.
- Absent-child schema generation raw0:
  `.pi/temporary/c4447fdd-845c-4985-91eb-958f7a51a276/`; reviewed JSON delta — только
  CardTitle/CategoryName.pattern в двух application schemas. Остальные схемы неизменны.
- GREEN `.pi/temporary/3266e9b3-e686-442e-910d-cf26dffc87ca/result.json`:
  Rust knowledge3/3, Ajv selected S05 test1/1 на24 fixtures, Python shared schemas,
  Rust schema --check, fresh absent TS generation/--check, protocol-only clippy/fmt,
  git diff --check — каждый raw0. Четыре generated TS файла byte-unchanged;
  broad SDK/foundation/workspace runtime не повторялся.
- Report P2: historical native fail-fast теперь отдельно UNKNOWN/NOT_DIAGNOSED,
  не «исправленный crash», не объединён с S03. Fresh reviewer exact receipt потреблён;
  later affected GREEN не выдаётся за causal proof.

Targeted independent closure `d659961a-b74a-4d64-aeb9-2ec3779a0c71` — substantive
**pass/findings[]**: `.pi/temporary/f4466f70-102c-4e3f-9c93-9fde53afab37/result.json`
с Rust3/3/Ajv1/1/Python raw0, generated inspection и source_changed=[]. Но delivery
workflow `daba1f01` завершился **FAIL**: `Required file-only output was not produced`.
Этот FAIL не переписан как PASS; partial diff/status/index/ref и raw report сохранены
в `.pi/recovery/s05-followup-infra-d659/`.

Same-native report-only settlement `40e05c7b-923e-4b12-b246-d492ff59fa0f`/workflow
`67726e54` восстановил исходный inline+reviewSchema contract; COMPLETE, bound verdict
**pass/findings[]**, потреблён parent. Bound report:
`C:/Users/User/.pi/agent/sessions/--C--PY-ScreenMemory--/subagent-artifacts/outputs/67726e54-7ce9-4b95-a3be-a3504d29ce04/s05-manual-p2-settlement/review.json`.
Новых source edits/runtime tests в settlement нет. Оригинальный fresh review **fix**
сохранён отдельно `.pi/recovery/s05-manual-core-a74f/original-review-a74f.json`.

Parent принял только назначенный связный manual slice и осмотренный own-HWND rendered
proof; SSR/GuiSession не genuine input. Исправление двух P2 и report settlement не
закрывают OCR/vision или whole S05.

Code intelligence: Codebase owners/calls KnowledgeApi→StorageWorker→handle_authorized/
transaction/source; ast-grep подтвердил один `hook.on_source_changed(tx, ...)` внутри
persist до COMMIT. Serena references — известное разрешённое `activeLS=[]`/No language
servers; current-source/graph fallback, не runtime доказательство. Статический graph
местами шумен; решающие fences/source facts сверены с actual code и tests.

Не реализованы/NOT_RUN: OCR identity/worker/package registration, реальный vision/prompt/
repair/AiRun/fenced publisher, preferences/context/privacy enrollment, live models/80-image
quality, FTS5/MCP/installer. Maintenance>4096 assigned records явно refused; 10/50k perf
и native genuine input остаются NOT_RUN/NOT_PROVEN. SDK protocol/distribution не менялись,
новых SDK/full-workspace/foundation аудитов нет. Никакого embeddings/vector/sqlite-vec,
скачивания моделей или изменения чужих/живых данных. Следующий owner — OCR/vision,
после этой scoped приёмки manual-core. Production FTS5 может использовать принятый
source seam независимо от локально заблокированных live AI/identity операций.
