# S02 — блокер изолированного Windows live-стенда

2026-10-02. Архитектурное решение, **не runtime PASS и не отказ product capture**.
Foundation scoped code `99df8f3` принят; полная S01/S02 не принята.

## Решение и evidence

Native Astra consultation: workflow `e7c30bf9-0e73-4905-95df-58c04b660294`,
child `b5e6c906-4118-4094-9478-60c9af48222f`, outcome `blocked`.
Support/integration/review children **не запускались**, source writer не назначен.
Durable decision: managed `s02-isolation-oracle/decision.json` указанного workflow.

Oracle сверил actual `crates/capture/src/gdi.rs`: публичный freeze использует
`GetDC(NULL)` и BitBlt whole virtual rect. У API нет установленной в этом proof
неизменной связи source DC с собственным HDESK после смены input desktop.
`SendInput` также не адресуется конкретному HWND/HDESK. Предварительные guards,
последующий discard и synthetic HWND не отменяют уже выполненное чтение/ввод.
Parent не разрешил operational race с личным Default; hidden-desktop эксперимент,
secure bypass, BlockInput и AttachThreadInput не являются альтернативой.

Read-only OS availability preflight, raw **0**:
`.pi/stand-availability-7121f4a126c24822a9358b7e3fc6d926/result.json`.
`WindowsSandbox.exe` отсутствует; fixed `Containers-DisposableClientVM` query
успешен, InstallState=2 (disabled). Это **BLOCKED**, не passing runtime stand.
VM inventory не перечислялся; отсутствие иных VM не утверждается.
Pixels/clipboard/input/process launch/feature changes — **NO**.

## Следующий разрешённый путь

Нужен явно собственный чистый Windows guest/session с только synthetic данными;
input/capture исполняются внутри guest, не в личном host/RDP-окне. Никаких личных
shares/clipboard redirection/models. OS features/global setup и чужие VM не менять.
После подтверждения stand — create-only UUID desktop, fresh children без inherited
handles, actual station/thread/input guards на каждом worker, opaque full physical
background, own watchdog, bounded 45/60 s и restore только из своего input desktop.
Это deadlines действий, не hard-real-time гарантия ОС; unexpected desktop => refusal.

Real four modes/hotkey/menu/first drag/Esc/click и capture→Saved→visible preview
остаются **BLOCKED/NOT_RUN**. Clipboard требует собственной station/session/VM;
все WinSta0 desktops делят clipboard. Новая feature opt-in не isolation proof.

Блокер принадлежит live-операциям, не всей цели. Продолжаются независимые S03
codecs/import/export, S04 bundled runtime и S06 FTS5. File import→real preview
доказывается отдельно и **не заменяет** capture proof. Не возвращаться к broad
recovery/credential audit только из-за отсутствия стенда.
