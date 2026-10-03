import { invoke } from '@tauri-apps/api/core';
import { useRef, useState } from 'react';
import type { ExportReceipt, RecordMetadata } from '../../../packages/protocol/src/generated/application-v1';
import type { ExportRequest } from '../../../packages/protocol/src/generated/application-methods-v1';
import type { LibraryRecord } from './model';

export default function ExportPanel({ record }: { record: LibraryRecord }) {
  const [format, setFormat] = useState<ExportRequest['format']>('png');
  const [path, setPath] = useState('');
  const [maxSide, setMaxSide] = useState('');
  const [background, setBackground] = useState('');
  const [receipt, setReceipt] = useState<ExportReceipt | null>(null);
  const [metadata, setMetadata] = useState<RecordMetadata | null>(null);
  const [notice, setNotice] = useState('');
  const [busy, setBusy] = useState(false);
  const pending = useRef(false);
  const request = useRef<ExportRequest | null>(null);
  const [operation, setOperation] = useState('');
  async function submit(retry: boolean) {
    if (pending.current) return;
    if (!retry) {
      const rgb = background ? background.split(',').map(Number) : null;
      if (rgb && (rgb.length !== 3 || rgb.some((n) => !Number.isInteger(n) || n < 0 || n > 255))) { setNotice('Фон JPEG: три числа 0–255 через запятую.'); return; }
      request.current = { operation_id: crypto.randomUUID(), record_id: record.id, asset_id: record.preview_asset_id, format, max_side: maxSide ? Number(maxSide) : null, jpeg_background: rgb as [number, number, number] | null, destination_path: path || null, metadata_policy: 'strip_private' };
      localStorage.setItem('sm-export-request', JSON.stringify(request.current));
    } else if (!request.current) {
      try { request.current = JSON.parse(localStorage.getItem('sm-export-request') ?? 'null') as ExportRequest | null; }
      catch { setNotice('Нет сохранённого export request.'); return; }
    }
    const value = request.current;
    if (!value || value.record_id !== record.id) { setNotice('Нет export request для этой записи.'); return; }
    setOperation(value.operation_id); setReceipt(null); setNotice(''); pending.current = true; setBusy(true);
    try {
      const answer = await invoke<ExportReceipt>('gui_export', { request: value });
      setReceipt(answer);
      setNotice(answer.partial ? 'Экспорт частично завершён.' : 'Экспорт завершён.');
    } catch { setNotice('EXPORT_UNCONFIRMED · повтор использует тот же operation UUID и параметры.'); }
    finally { pending.current = false; setBusy(false); }
  }
  async function loadMetadata() {
    try { setMetadata(await invoke<RecordMetadata>('gui_metadata', { request: { record_id: record.id } })); }
    catch { setNotice('METADATA_UNAVAILABLE'); }
  }
  return <section aria-label="Экспорт и metadata">
    <h3>Экспорт (канон не изменяется)</h3>
    <label>Формат<select value={format} onChange={(e) => setFormat(e.target.value as ExportRequest['format'])}>{['png', 'jpeg', 'webp', 'avif'].map((f) => <option key={f}>{f}</option>)}</select></label>
    <label>Назначение (абсолютный путь; пусто = временный export на 24 часа)<input value={path} onChange={(e) => setPath(e.target.value)} /></label>
    <label>Максимальная сторона<select value={maxSide} onChange={(e) => setMaxSide(e.target.value)}><option value="">Без resize</option>{[320, 1000, 1024, 2048].map((n) => <option key={n}>{n}</option>)}</select></label>
    <label>Фон JPEG (например 255,255,255)<input value={background} onChange={(e) => setBackground(e.target.value)} /></label>
    <button disabled={busy} onClick={() => void submit(false)}>Экспортировать</button>
    <button disabled={busy} onClick={() => void submit(true)}>Повторить тот же экспорт</button>
    {operation && <p>Operation UUID: {operation}</p>}
    {notice && <p role="status">{notice}</p>}
    {receipt && <p>{receipt.mime} · {receipt.width}×{receipt.height} · {receipt.byte_size} байт · {receipt.existence} · {receipt.export_path} · SHA256 {receipt.sha256}</p>}
    <button onClick={() => void loadMetadata()}>Показать metadata источника</button>
    {metadata && <details open><summary>{metadata.source_kind} · {metadata.record_id}</summary><pre>{metadata.source_reports.join('\n')}</pre></details>}
  </section>;
}
