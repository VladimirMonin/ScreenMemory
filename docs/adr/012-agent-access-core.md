# ADR-012 — ограниченный агентский доступ через общий host

- Решение parent в границах подтверждённой цели `muqhdldy-5nzeny`, 2026-10-04.
- Статус: **контракт назначенного среза; реализация и приёмка ещё не выполнены**.
- Основа: [master §12.5](../ScreenMemory_Full_Development_Plan_v3.md#integrity),
  [S07 §9 / CN-S07-02](../ScreenMemory_S07_GUI_CLI_MCP.md),
  [ADR-005](005-local-credentials.md), [ADR-008](008-session-client.md),
  [scope ADR-011](011-private-release-scope.md).

## Назначенный пользовательский сценарий

Владелец через authenticated host создаёт metadata-only профиль и connection.
`shot --config <own-config> --connection <id> search ...` и чтение разрешённой
карточки используют общий ApplicationClient, отдельный credential и persistent
principal. После отзыва тот же token/session/cursor больше не выдаёт данные;
owner и другие подключения продолжают работать. Missing/revoked connection
никогда не переключается на owner.

Это первый полезный S07 access/CLI срез, **не полный S07 или готовый MCP**.
Pixels/resources, остальные права/allowed roots, MCP SDK/profiles, GUI access
management и clean-machine acceptance остаются обязательным следующим scope.

## Интеграционные решения

1. `sm-storage` остаётся единственным SQLite owner. Нужная additive migration —
   **0009** после фактической 0008, а не историческая plan 0007. Принятые SQL
   0001–0008/checksums, library UUID, канон, source и Saved не меняются.
   Профили, версии, connections, enrollment intent/digest, receipts/events —
   typed bounded port того же worker, не новая auth DB.
2. Default и первоначально поддержанный grant — только `read_metadata`;
   roots/capture modes пусты. Неподдержанные grants/actions отклоняются явно,
   а не сохраняются как будто уже enforced. Остальной scope S07 не исключён.
   Limits: profiles64, connections128, name1…80 Unicode scalars; закрытые
   states/revisions/policies, immutable principal и revoke terminal state.
3. Credential — CSPRNG, versioned secret вне SQL/JSON/config/argv/logs.
   Разрешается **typed** exact key extension
   `<environment-namespace>/library/<library-id>/agent/<connection-id>`;
   произвольный target/string, enumeration и owner/backend fallback запрещены.
   Native adapter проверяет прежний Environment. TestReadOnly не agent profile.
4. Enrollment: stable operation/fingerprint/connection и ожидаемый digest commit
   до SecretStore write; readback exact digest, затем activation/receipt. Crash/replay
   сохраняют те же IDs; неизвестный secret не усыновляется. Нет общей транзакции
   SQL и SecretStore. Revoke/disabled/security revision сначала прекращают
   authorization, затем exact slot cleanup; cleanup error не возвращает права.
5. Agent authority проверяется при handshake и последующих requests/delivery,
   а не immutable scope snapshot только на connect. Security revision и credential
   generation входят в binding; старые sessions/cursors не возрождаются после
   disable→enable/reconnect. Реальный порядок commit/authorization publication и
   delayed response/revoke проверяется тестом, не обещанием общей атомарности.
6. Scope/visibility применяется до cardinality/snippet/page/card/catalog. Любой
   missing required source scope исключает всю запись. Глобальные labels/counts
   knowledge category catalog не выдаются metadata-only agent: нужна eligible
   projection либо явный безопасный refusal, не дамп owner catalog.
   Legacy Owner-only ListRecords не расширяется; agent browse — общий SearchApi.
7. `--connection` выбирает credential только данного connection. Этот режим
   требует уже работающий desktop host, не использует privileged bootstrap,
   enrollment или owner token. Parser сохраняет прежние literals/commands,
   closed unknown/missing/duplicate rejection и один JSON/error exit contract.
8. Saved retention, pending operations, source scopes и processing publication
   fences не ослабляются. Revoke отменяет свои waits/выдачу, не owner capture,
   общий host, durable jobs, SDK или модели.

## Проверка и полномочия

Только fresh owned synthetic roots, handles и exact Test credentials; raw exits,
actual source guards и отдельное cleanup/absence evidence. Реальные session/host и
shot subprocess доказывают assigned flow; fake store нужен для crash/SQL faults,
но не называется native vault proof. Offline pinned toolchain/cache; без новых
packages/downloads, GUI/CDP/третьего window pass, личных файлов/clipboard/model calls,
OS/cert/installer mutation или push. Parent принимает после fresh read-only review.

Полный stage остаётся IN_PROGRESS. Этот note принимает названный integration
contract CN-S07-02 на фактической schema9 после реализации; прочие CN-S07 не
становятся автоматически принятыми, а исторические девять планов неизменны.
