# S01 foundation — исправление P1 cleanup, 2026-10-02

Scope: developer helpers, не production storage/IPC и не полная S01 acceptance.
Base: `9a7944d5cf33563ccb598e5b7e9dc035d8190e4e`. Parent принимает код после fresh review.

## Подтверждённый дефект

Fresh Sol6.1 review foundation seam вернул `fix`: отвергнутый GUI config A мог
авторизовать `finally` cleanup library B. Валидный direct-CredWrite repro:
`.pi/temporary/82467f3d-a1a6-4f7c-8a1d-12141a257165/repro.json`, raw 1,
B credential `0 → 1168`, root A удалён до connect. Его exact synthetic keys/roots убраны.
Parent red baseline `.pi/temporary/7c85bcba-37e7-4c2f-9042-9ecce3a50397/result.json`
также показал удаление A/B-key; host-count поле этого первого harness было неверно
обёрнуто nested array и исправлено. Primary deletion proof от этого не зависит.

## Решение и реализация

Native Astra oracle согласовал independent init-issued binding, не selfcheck authority.
`ApplicationClient::connect` подтверждает library/image, но не filesystem root.
Consultation child завершён; wrapper `9762262e` упал на `emit(undefined)`, не PASS.
Diff/status сохранены `.pi/recovery/foundation-oracle-wrapper-9762262e/`;
same-native-protocol resume завершён workflow `3832793e-6442-4944-98af-6332cef919e1`.
Это advice, не code acceptance; модель/режим не менялись.

- Create-only `.screenmemory-preview-library.json` `{directory_uuid, library_id}`
  из собственного успешного `shotd --init` нового root. Не из `preview.json`/legacy.
- `Resolve-ValidatedPreview`: marker/tree, binding, library, schema/test, normalized
  root и trusted image. Один validator для GUI и общего CLI cleanup.
- Arm до connect; frozen tuple + hashes. Отказ preflight — никаких stop/delete.
- Mutation после arm — REFUSED, root/key/host сохраняются с локальным BLOCKED;
  окно закрывается только по own handle. Неизменные inputs сохраняют error cleanup.
- Guard не является sandbox против согласованной подделки всех файлов и не даёт
  атомарности против hostile same-user TOCTOU. Production канон/lease не упрощены.

## Реальные проверки (private immutable artifacts)

| Проверка | Результат |
|---|---|
| Two-root/config/binding/mutation | 12/12, raw 0; `.pi/temporary/2761cdb2-8be3-4e8e-b7f1-3b3be11e4347/result.json` |
| Existing cleanup safety | 65 checks, raw 0; `.pi/temporary/41469801-1bf2-4200-9a68-4d76e414961e/result.json` |
| Post-P2 composed host/CLI/GUI/error/isolation | 6/6, raw 0; `.pi/temporary/95051d2a-990e-410e-8dfa-a6435f974123/result.json`; оба smoke keys независимо absent1168 |
| Physical owned GUI | 1462×1136, DPI144, ready/empty synthetic library; `.../b18036ea-ffc3-4307-af94-1634ef22f750/GUI-WINDOW/window/owned-window.png`; SHA256 `6243fa4209535a391581a508af1cd95eb550c58e59f0ac257674241b9c5689be` |

Mutation probe `761301f1` дал функциональный PASS, но ошибочно пытался hash locked
live-host files; это НЕ live-file integrity proof. Исправлен scope snapshot, clean
12-case artifact выше. Composed `ad72b9b9` raw 1 выявил старое lock-absence assertion
на теперь initialized fixture; проверка заменена честным before/after hash init-created
lock/database, затем свежий 6/6 PASS. Старые artifacts не переписаны.

Исторический malformed reviewer canary: `UNKNOWN/NOT_RECONCILED`; две узкие
TargetName-only reconciliation не нашли exact match, ничего не удалили, поиск остановлен.
Это не разрешение перечислять/чистить остальные Test credentials.

## Независимый review и P2

Fresh Sol6.1 `7fb13900` подтвердил закрытие P1/12-case PASS, но нашёл P2:
success smoke удалял второй initialized root без credential cleanup. Исправлено
через общий ordered cleanup, проверку ok/readback1168 и отдельный exact native
absence1168; private init identity записывается до destructive paths. Новая
parent composed suite `95051d2a` raw0; прежний `b18036ea` **не** full credential-disposal
proof, возможный собственный residue UNKNOWN (ID не сохранялся; root уже удалён).
Никакой enumeration/reconciliation неизвестных keys не выполнялось.

Same-role Sol6.1 challenge `f715cc2b`, workflow
`217e08f1-03fb-4587-91cd-13194ecdffac`: **accept**, P1/P2 closed, findings0;
`.pi/temporary/1b406900-3894-4dd4-ada5-05558f56a916/evidence/result.json` — native
readback **1168 для обоих свежих exact parent IDs**, own hosts0/rootsgone.
Failure-path conclusion source-only; нового injected-negative P2 runtime не заявлено.

Parent post-P2 verify: **36 gates raw0**,
`.pi/temporary/cd649d62-c5d9-4794-b518-0cacd75eb9cf/result.json`, plan/source PRE/POST PASS.
Runner regression raw0: `.pi/test-verify-33b79ace36b5467bbf960b51a58bc22d/result.json`;
предыдущий `1b915723` raw1/empty PREREQ-pnpm logs сохранён как transient failure,
прямой pinned pnpm10.32.1 raw0 и unchanged-source repeat raw0; bypass не было.

Parent verdict: **ACCEPT_CODE_SCOPED**, foundation boot/verification/helper seam.
Whole S01 `IN_PROGRESS`; live capture/clipboard/AI/package `NOT_RUN`, vectors `EXCLUDED`.
