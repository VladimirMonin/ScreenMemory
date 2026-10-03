import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { useEffect, useRef, useState } from 'react';
import type { ImportReport } from '../../../packages/protocol/src/generated/application-v1';
import type { ImportRequest, ImportControlRequest } from '../../../packages/protocol/src/generated/application-methods-v1';
import { canonicalAssetId } from './model';

export default function ImportPanel({ onSaved }: { onSaved: () => void }) {
  const [paths, setPaths] = useState('');
  const [profile, setProfile] = useState<ImportRequest['profile']>('webp_exact');
  const [duplicates, setDuplicates] = useState<ImportRequest['duplicates']>('reuse_record');
  const [batch, setBatch] = useState(() => localStorage.getItem('sm-import-batch') ?? '');
  const [report, setReport] = useState<ImportReport | null>(null);
  const [notice, setNotice] = useState('');
  const [busy, setBusy] = useState(false);
  const pending = useRef(false);
  const currentBatch = useRef(batch);
  const reportRequest = useRef(0);
  const savedCount = useRef(-1);
  const reportRevision = useRef(0n);
  const savedCallback = useRef(onSaved);
  savedCallback.current = onSaved;

  function accept(value: ImportReport) {
    if (value.batch_id !== currentBatch.current || !/^(0|[1-9][0-9]*)$/.test(value.revision)) return;
    const revision = BigInt(value.revision);
    if (revision < reportRevision.current) return;
    reportRevision.current = revision;
    setReport(value);
    const count = value.saved_new + value.linked_existing;
    if (count > 0 && count !== savedCount.current) savedCallback.current();
    savedCount.current = count;
  }
  function useBatch(id: string) {
    currentBatch.current = id;
    reportRequest.current += 1;
    savedCount.current = -1;
    reportRevision.current = 0n;
    setBatch(id); setReport(null);
    localStorage.setItem('sm-import-batch', id);
  }
  async function refresh(afterOrdinal = 0) {
    const id = currentBatch.current;
    if (!canonicalAssetId(id)) { setNotice('Нужен UUIDv4 batch.'); return; }
    const generation = ++reportRequest.current;
    try {
      const value = await invoke<ImportReport>('gui_import_report', { request: { batch_id: id, after_ordinal: afterOrdinal } });
      if (generation === reportRequest.current && id === currentBatch.current) accept(value);
    } catch { if (generation === reportRequest.current) setNotice('IMPORT_REPORT_UNAVAILABLE · batch сохранён, можно повторить.'); }
  }
  async function start() {
    if (pending.current) return;
    const sourcePaths = paths.split(/\r?\n/).map((p) => p.trim()).filter(Boolean);
    if (sourcePaths.length === 0 || sourcePaths.length > 128) { setNotice('Укажите 1–128 абсолютных файлов/папок, по одному пути на строку.'); return; }
    pending.current = true; setBusy(true); setNotice('');
    const id = crypto.randomUUID(); useBatch(id);
    // ID exists before invoke; a lost answer never causes a second batch on refresh/retry.
    const request: ImportRequest = { batch_id: id, source_paths: sourcePaths, profile, duplicates };
    try { accept(await invoke<ImportReport>('gui_import', { request })); }
    catch { setNotice('IMPORT_UNCONFIRMED · проверьте отчёт этого batch, не создавайте его повторно.'); }
    finally { pending.current = false; setBusy(false); }
  }
  async function control(control: ImportControlRequest['control']) {
    if (pending.current || !canonicalAssetId(currentBatch.current)) return;
    pending.current = true; setBusy(true);
    try { accept(await invoke<ImportReport>('gui_import_control', { request: { batch_id: currentBatch.current, control } })); }
    catch { setNotice('IMPORT_CONTROL_UNCONFIRMED · проверьте отчёт.'); }
    finally { pending.current = false; setBusy(false); }
  }
  useEffect(() => {
    let disposed = false;
    let unlisten: (() => void) | undefined;
    void listen<string[]>('gui-import-paths', (event) => {
      if (!disposed) { setPaths(event.payload.join('\n')); setNotice('Пути получены native drop. Выберите профиль и нажмите «Импортировать».'); }
    }).then((stop) => { if (disposed) stop(); else unlisten = stop; });
    return () => { disposed = true; unlisten?.(); reportRequest.current += 1; };
  }, []);
  useEffect(() => {
    if (!canonicalAssetId(batch)) return;
    void refresh();
    const timer = setInterval(() => { if (!pending.current && (!report || report.active_work)) void refresh(); }, 1500);
    return () => clearInterval(timer);
  }, [batch, report?.active_work]);
  return <section aria-label="Импорт файлов и папок" className="import-panel">
    <h2>Импорт файлов и папок</h2>
    <p>Перетащите файлы/папки в окно или введите локальные абсолютные пути. Обход и обработка выполняются host; исходники не изменяются.</p>
    <label>Пути (по одному на строку)<textarea value={paths} onChange={(e) => setPaths(e.target.value)} rows={3} /></label>
    <label>Канон<select value={profile} onChange={(e) => setProfile(e.target.value as ImportRequest['profile'])}><option value="webp_exact">WebP · без потерь</option><option value="avif_sdr90">AVIF · SDR quality 90, с потерями RGB</option></select></label>
    <label>Дубликаты<select value={duplicates} onChange={(e) => setDuplicates(e.target.value as ImportRequest['duplicates'])}><option value="reuse_record">Связать с существующей записью</option><option value="keep_both">Сохранить обе</option></select></label>
    <button disabled={busy} onClick={() => void start()}>Импортировать</button>
    <label>Batch UUID (сохранён между открытиями)<input value={batch} onChange={(e) => useBatch(e.target.value)} /></label>
    <button disabled={busy} onClick={() => void refresh()}>Обновить отчёт</button>
    <button disabled={busy || !batch} onClick={() => void control('pause')}>Пауза</button>
    <button disabled={busy || !batch} onClick={() => void control('cancel_processing')}>Отменить обработку</button>
    <button disabled={busy || !batch} onClick={() => void control('resume')}>Продолжить</button>
    <button disabled={busy || !batch} onClick={() => void control('retry')}>Повторить из удержанного input</button>
    {notice && <p role="status">{notice}</p>}
    {report && <div aria-live="polite">
      <p>{report.state} · найдено {report.discovered} · Saved {report.saved_new} · связано {report.linked_existing} · пропущено {report.skipped} · ошибки {report.failed} · удержано {report.held} · input {report.retained_input} · в работе {report.in_flight}</p>
      <p>Обход: {report.discovery_complete ? 'завершён' : 'не завершён'} · ошибки каталогов {report.directory_issues}{report.discovery_limit_hit ? ' · достигнут лимит, требуется Continue' : ''}{report.discovery_abandoned ? ' · отменён' : ''}</p>
      <ul>{report.items.map((item) => <li key={item.item_id}>#{item.ordinal} · {item.state}{item.safe_code ? ` · ${item.safe_code}` : ''}{item.receipt ? ` · ${item.receipt.width}×${item.receipt.height} · ${item.receipt.canonical_mime}` : ''}</li>)}</ul>
      {report.next_ordinal !== null && <button onClick={() => void refresh(report.next_ordinal ?? 0)}>Следующие результаты</button>}
    </div>}
  </section>;
}
