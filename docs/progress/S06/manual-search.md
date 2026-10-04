# S06 — manual FTS5: проверенный код / runtime BLOCKED

2026-10-04. **CODE_CHECKPOINT_SCOPED; S06 IN_PROGRESS, не принят.** Parent фиксирует
проверенный manual3-field код после независимых scoped reviews, но не принимает
required nonblank render/runtime gate или полный S06. Goal остаётся 3/9.
Original full assignment и retained closure завершились timeout; NOT_EVALUATED,
partial/not-satisfied и исходные FAIL сохранены. Последний bounded sensor pass дал
содержательный criterion-1 not-satisfied, не timeout/parser failure. Это отдельный
известный live-gate blocker, не разрешение отключить assertion или назвать этап ready.

## Реализованный полезный срез

Saved/import/recovery → admitted `manual_effective_v1` → atomic contentful FTS5/exact/epoch
→ общий SearchApi/ApplicationClient → JSON CLI и SearchPanel. Current означает только
согласованную проекцию реально предоставленных title/description/category.
Application/window/primary filename/OCR, taken/captured/applied-AI publishers остаются
NotProvided; соответствующие фильтры Unsupported до SQL. Нет embeddings/семантики,
моделей/SDK в WebView или скрытого model call от запроса.

Retained correction: один active + один replaceable latest pending в renderer, без busy
polling/retry; monotonic generation и очистка stale page/error/preview. Native SearchLane
принимает одного caller; cancellation token достигает active/queued caller **вне**
GuiSession mutex. Shared client пишет существующий connection-local Cancel и дренирует
terminal response, не прерывает writer/чужой connection и не переиспользует poisoned IO.
Readonly catalogue/selection и stored preview остаются прежними scoped services.

## Проверки этого follow-up

Raw command/result/stdout/stderr SHA — private UUID `.pi/temporary/`, не публичные логи:

- Mounted ReactDOM/StrictMode delayed A→B→C, только C admitted, stale error/page/preview,
  unmount/cleanup; frontend28/28/lint/build raw0: `f95bf02e`, `81306bb9`, `f414f1d7`.
  Final type/lint/test/build raw0 `3db51ddf`/`9dc84e7a`/`eb67a070`/`bf33d963`:
  mounted3/3 и total29/29, включая expired cleanup/ready-loss. Native lane queued+active2/2
  `eab40459`, final affected all-target clippy `a7e09525`, fmt-check `a14b3273` raw0.
  Auxiliary post-build bundle text scan отказал на cp1251 decoding: исходный FAIL сохранён.
  Parent отдельно прочитал final dist как UTF8: named markers happy-dom/happyDOM/
  webidl-conversions/whatwg-mimetype отсутствуют; это узкий scan вместе с actual lint/
  source dev-only boundary, не универсальное доказательство состава бинарника.
  Parent consumption9 raw0 receipts + stdout/stderr hashes:
  `.pi/temporary/8c411f92-fe7c-468b-9114-564439096c80/parent-consumption.json`.
  Installed three-package versions/licenses и React19.2.0 подтверждены отдельно.
- Native named-pipe actual cancellation: A terminal-drained, connection B не отменён,
  next ordinary RPC на A работает — `54ef84d3` raw0. Native lane scope/admission —
  `5bd1b2a2` raw0; queued-before-lock seam дополнительно проверяется final unit gate.
- Affected GUI/session/storage **all-targets** clippy с explicit window feature raw0
  `09212263`; default production binaries `55a0f466` raw0. Headless actual default
  Saved/CLI/GuiSession/canonical/reopen/export/ordinary edit/preview `9905d8ff` raw0,
  окно в нём честно NOT_RUN.
- Required tracked runtime gate:
  `cargo test --offline --locked -p sm-gui --features owned-window-tests --test manual_knowledge -- --test-threads=1 --nocapture`.
  Первый разрешённый follow-up window pass **FAIL raw101** `e2b7edfc`: prerequisite0
  до fixtures, exact GUI/image/config/library/creation/argv guards0, selected-port
  listener отсутствовал (`endpoint` raw1, `CmdletizationQuery_NotFound`) до CDP.
  Exact outputs `.pi/temporary/71bb720c-b568-4d88-9a32-49d26af04c54/` сохранены.
  Native Astra consultation `ebafb0ba` и actual supervisor reply различили доказанное
  отсутствие в одном наблюдении после sleep(5s) и неподтверждённую readiness-гипотезу.
  Env-loss/profile reuse/policy не доказаны. Parent отдельно разрешил только минимальный
  test-only sensor и **ровно один новый pass #2**, не production WebView fix.
  Fresh current native build raw0 (8.15s), pure7/7 и exact fmt/syntax GREEN.
  Pass #2 `9aaf4059` **FAIL raw101**, 0/1; в одном owned GUI lifetime восемь
  identity raw0 и exact same-port `CmdletizationQuery_NotFound,Get-NetTCPConnection`
  raw3, затем общий deadline **до CDP**. Receipt elapsed15.0002792s включает отказ/
  запись, не hard-real-time гарантию; поздний owned result не допускается.
  `.pi/temporary/07f87c08-6d2c-49b9-8ab7-cc12fac49da6/` — build/native raw receipts;
  `.pi/temporary/604ab2e0-3098-4102-9dea-6b90027e0afe/` — восемь observations,
  readiness refusal, payload receipt отсутствует. До window gate actual CLI import/
  metadata/manual/search прошли; tail RPC/edit/export/reopen после assertion NOT_RUN.
  Managed criterion-1 not-satisfied, SOURCE_RELEASED/READY=false; root workflow FAILED.
  **Nonblank rendered search→select→stored Blob/SHA/preview BLOCKED/NOT_PROVEN**;
  deeper cause UNKNOWN. Третьего/альтернативного запуска не было и не разрешено.
- В failed window pass own WM_CLOSE+GUI wait raw0, authenticated host stop+wait raw0,
  exact credential/root/source absence подтверждены. Nonpanicking RAII сохраняет
  исходный assertion; cleanup PASS не превращает runtime failure в PASS.
  Pass #2 также завершил GUI WM_CLOSE+wait0, authenticated host stop+wait0 и проверил
  exact credential/root/source absence; native stdout assertions сохранены отдельно от
  общего raw101. Parent открыл physical1462×1136 own PrintWindow: текст/два browse cards
  читаемы, query пустой, selected preview отсутствует. Это browse-only, не требуемый gate.
  PrintWindow own HWND/DPI — отдельно от DOM и genuine input; physical input BLOCKED.
- Fresh read-only Sol reviews: `7e9d233d` pass/findings[] закрыл два исходных P2
  cancellation/latest и ignored-helper/prerequisite; независимые serial native client1/1
  и GUI lane2/2 raw0 `.pi/temporary/93889623-4a61-4f9b-a274-94b597d4c17b/`.
  После sensor delta `90a7f1d3` pass/findings[], независимые tracked pure7/7 raw0
  `.pi/temporary/fce6912e-eefe-45c8-824a-ea0f9912eefd/`; first PowerShell wrapper raw1
  из-за NativeCommandError на unittest stderr сохранён, direct same-native shell raw0.
  Reviews не запускали новое окно и не переоценивали criterion-1/full S06.
  Pure baseline raw1 (import/argparse) — failure-before-test, не behavioral TDD-red.
- Parent final diff нашёл дополнительный CLI P2: значение `search` в
  `knowledge edit --title search` ошибочно выбирало search parser. Native `7b417eba`
  заменил any-argv detection выбором top-level command; literal title/description,
  global ordering, strict unknown/missing/duplicate flags и typed start/page/cancel
  покрыты regression. Behavioral RED raw101 сохранён. Первый unfiltered CLI gate
  raw101 остановился на прежнем test oracle schema7/actual8; skip15/1 raw0 был только
  partial proof, не принят за полный PASS. Same-native `9b8f842e` обновил **только**
  strict oracle на8: unfiltered six targets **16/16 raw0**, без skip/ignored/filtered.
  Current CLI build/all-target clippy/fmt/version raw0; synthetic fixture closure и
  absence подтверждены, не capture/input proof.
  Fresh readonly `284ee3cc` actual native API verdict pass/findings[]; independent
  knowledge/search5/5 + exact native status/doctor/stop1/1 raw0,397/397 source hashes
  unchanged, index пуст. Его bound `review.json` содержит только introductory text,
  но schema-valid4-key `structured-output` и native status доставили настоящий verdict:
  обе версии сохранены, повторного reviewer или plugin patch не было. Receipts hash
  относятся к LF prewrite text; Parent отдельно сверил сохранённые CRLF bytes/hash.
  `.pi/s06-cli-schema8-730dac00-732e-4973-9999-6c27a3749eb2/` и
  `.pi/s06-cli-review-c320e2e7-6917-4c4f-99fc-e40adf5f23d8/` — raw evidence;
  `.pi/recovery/s06-cli-reviewed-284ee3cc-dde743bc-e1d7-4454-8835-e5041fdb2e19/`
  сохраняет actual API JSON/status/terminal/source/barrier. Это закрытие CLI P2,
  **не** required window criterion-1 или полный S06.

## Качество / производительность: не подменять доказательства

V1 immutable; V2 — ровно четыре parent-approved factual query corrections **после**
первого V1 runtime. Старые failures сохранены, tokenizer/веса/grammar/cards не fitted.
Existing120 literal queries/100 cards: Recall/Hit/MRR/nDCG1.0, exact20 top1,
negative20 без hits; это шаблонный literal regression, не универсальное качество.

Отдельно ДО первого evaluation заморожен `graded-supplement-v1.json`:
12 явно заданных source records,10 случаев (title vs description, OR, exact vs lexical
collision и действительно отсутствующий near-code),9 positives/1 negative, grades3/2/1,
explicit expected top1/forbidden IDs. SHA256
`ac552705a60ce67e0979be860db949a97403f865916d4e580c8dea40cabd1f4e`.
Parent проверил actual native chronology: fixture write02:20:43.346Z, successful write
result02:20:43.353Z и тот же SHA/3135bytes **до** first production invocation02:35:29.307Z.
Production shared-service pass `1e04cbc2` raw0; actual IDs/grades/metrics
`.pi/temporary/993d9912-9845-4b64-9821-0e3c37e50780/graded-results.json`:
Recall@5/nDCG@5=1.0,9 expected top1,zero forbidden/negative hits.
Первый многошаговый tool batch завершился raw101 из-за **последующего** native client
check `4db10048`, а не graded service; batch failure не переименован в PASS.
Поздний client `54ef84d3` и независимый `93889623` raw0 — отдельные проверки.
Rare category/scopes below125 unfiltered distractors — отдельно существующий core
regression; supplement не выдаётся за этот privacy proof или semantic/morphology quality.

Existing release10k production projection/SearchApi, не10000 Saved images:
SQLp95=82.1875ms/new-window230.5985/warm4.7213/writer-request upper-bound3.9011ms,
inflight cancellation+next ordinary save. Raw0 `c2e569fe`, metrics `80623e08`.
Debug FAIL и холодный first-sample-only лимит сохранены; perf/SDK/foundation не повторялись.

## Воспроизводимость и test-only dependency closure

Required helpers поставлены в tracked `tests/gui/`, не ignored orchestration.
Explicit already-installed Python3.14.2 AMD64, Playwright1.58.0, jsonschema4.26.0;
`prerequisites.py` проверяет их/required helpers до DB/root/credential side effects.
Native test использует approved absolute interpreter, не PATH. Отсутствие prerequisites
— failure/NOT_RUN, не skip PASS. Playwright только attach к проверенному own WebView,
не browser launch/download. Default offline native target не открывает окно; nondefault
feature — отдельный **обязательный** runtime gate, не отменённый assertion.

Mounted tests: exact dev-only Happy DOM16.8.1 (MIT), webidl-conversions7.0.0 (BSD-2-Clause),
whatwg-mimetype3.0.0 (MIT). Cached closure из project-owned donor cache прочитана readonly:
manifest/license/version + каждый CAFS member SRI/mode/path проверены; bounded create-only
copy в прежний primary store, indexes last, no hardlinks/overwrite. Это provenance cached
content, **не** проверка отсутствующего official tgz. Receipt `ff3be79c` raw0.
Original offline metadata/no-TTY setup failures сохранены (`55150232`,`ffbfef2f`).
Parent-approved exact lock delta содержит только3 package records и GUI optional
Vitest3.2.4 peer clone с прежними baseline versions. Semantic lockcheck `9ad869a2` raw0;
frozen offline ignore-scripts materialization `6ac8f051` raw0, lock SHA unchanged
`0473ff3cf42b91b192510d5082c90f568c96fb2d3e27c31eafe5aa6068dd38eb`.
Никаких donor app/runtime/tests, сетевых hooks/download/global config/newstore.
Happy DOM — только synthetic trusted mounted tests: scripts/resources/iframe/navigation
отключены, fetch/XHR отказывают, untrusted HTML/eval отсутствуют; production bundle не
импортирует DOM library. Это не замена actual native/render gate.

## Остаток и следующий шаг

Bounded closure **partial/not-satisfied** пока required visible runtime gate не зелёный.
Parent фиксирует только проверенный code checkpoint с явно отделённым live blocker:
это **не** managed CHECKED этих failed assignments и **не** accepted S06/release.
Native writers/reviewers завершены, source released. Helper review проверил86 paths,
его delta не изменил прочие82 файла. После CLI P2/oracle delta fresh review и Parent
проверили397/397 current source hashes; новый CLI delta затрагивает ровно args.rs,
knowledge_args.rs, search_args.rs и one-line cli_surface.rs, остальные bytes сохранены.
Index до Parent staging пуст. Полный curated diff и controlled V1→V2 annotation delta
прочитаны Parent; staged code checkpoint не равен stage acceptance.
Final recovery `.pi/recovery/s06-pass2-blocked-9aaf4059-3ab2b618-7597-40a3-9dc3-fa0b7ee64fc8/`
сохраняет ZIP/diff/SHA/normal partial/failed native+workflow receipts/release/cleanup.
Следующий полезный milestone — агентские общие CLI/MCP S07; не новый listener loop.
Нельзя заменить current FAIL историческим native PASS/SSR/browse или снять assertion.
Full S06/S05/S07, four missing source owners, genuine input/own clean stand/live model/
release остаются открытыми; пользователь не тестер. Для нового runtime пути требуется
обоснованное отдельное stand/authority решение. Protected nine plans/migrations0001–0007
неизменны; стадия не DONE, staging/commit выполняет только Parent.
