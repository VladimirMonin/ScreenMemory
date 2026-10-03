# S03 — runnable image owner (scoped, не весь этап)

Retained candidate от `6509b21e0edc24b2e57639d31c9c0720899ea402`, без restart.
Рабочий seam: static decode/metadata/orientation/color/resize/export и собственный
`shot-image-worker.exe`; trusted manifest pin, bounded framing, Job memory/child limit,
kill + OS wait + Job ActiveProcesses0 до release. Candidate/WorkerResult — не Saved.
Input snapshot сохраняет coordinator до dispatch; error/cancel его не удаляют.
API/actual schema4 handoff: [input-manifest.md](input-manifest.md).
Pinned source licenses/checksums: [dependencies.json](dependencies.json).

## Текущие runtime checks

Обычные UUID logs: `.pi/85edb647-ea99-447f-a0b0-8f8161d5aa59/`, рядом `*.exit`.
Все команды через существующий pinned `.pi/t09b-run.cmd`, offline+locked;
default test threads, без RUST_MIN_STACK/новых ignored/relaxed assertions.

| Команда | Результат |
|---|---|
| `test --offline --locked --workspace` | raw0; image17, production worker2; storage117 PASS/1 existing guarded child ignored; 60 test/doc runs PASS |
| `test --offline --locked -p sm-image-worker --features worker-test-seams --test worker` | raw0;3 PASS, включая current hard-timeout/crash/cancel/overload/Job-memory/termination case |
| `build --offline --locked -p sm-image-worker --bin shot-image-worker` | raw0; default production binary восстановлен после fault-feature build |

**20 CURRENT уникальных image/worker checks** = image17 + production2 + fault1.
Два production tests повторены также в feature run и второй раз не считаются.
Golden SHA/WebP alpha0 exact/actual AVIF444 alpha+lossy RGB, EXIF/GPS/time provenance,
orientation1–8/container conflict, ICC, TIFF4 compressions, stripped exports/JPEG444
background, 10000 resize bounds и explicit animation/RAW/HDR refusals — не mock codec.

## Прежний crash и ограничение вывода

Failed full verify `.pi/4d68a4d1-54e8-4a7b-bbbc-c3a5f74d9826/` остаётся FAIL:
sm-storage `0xc0000409`, cause **NOT_ESTABLISHED**, не «устранён исторический баг».
Независимый narrow diagnosis не назначил source fix; спекулятивный wrapper/frame rollback
не выполнен. Новый default workspace запуск прошёл тот же `sm_storage-e7e45aa4707a95a6`
executable, включая117 storage tests. Это runtime regression PASS, не причинное доказательство.
Старый fault-case PASS не используется вместо current feature run.

## Передача владельца

Image implementation завершена в scoped смысле и ownership освобождён для ingest;
независимый review/принятие/commit принадлежат parent. SQL/host/RPC/UI не изменены.
Source+nine-plan guards были PASS на предыдущем verify; новый полный PS verify здесь
не повторялся — один final verify принадлежит integrated milestone.
NOT_RUN/out of scope: clean-package/install, full96MP/RSS1GiB/500-cycle stress,
shared capture admission, import journal/DB/source snapshots, protected export
registry/TTL, real GUI/drop и full S03/S02 capture acceptance. Recovery1MP cap прежний,
Saved/canonical/no-replace/root/credential authority не ослаблялись; capture stand
блокер локален, не заменён synthetic image tests.
