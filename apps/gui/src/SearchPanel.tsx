import { invoke } from '@tauri-apps/api/core';
import { useEffect, useRef, useState } from 'react';
import type { CategoryCatalog, SearchHit, SearchPage, SearchReply } from '../../../packages/protocol/src/generated/application-v1';
import type { SearchFilters, SearchRequest } from '../../../packages/protocol/src/generated/application-methods-v1';
import { canonicalAssetId, previewBytes, type AssetPreview } from './model';
import KnowledgePanel from './KnowledgePanel';
import { LatestSearchLane } from './searchLane';

/** Monotonic request IDs, NOT timestamps. Used by the actual async renderer path. */
export class SearchGeneration {
  private generation = 0;
  next() { return ++this.generation; }
  current(id: number) { return id === this.generation; }
}
export function searchFilters(category: string, source: string): SearchFilters {
  return { category: category === 'uncategorized' ? { kind: 'uncategorized' } : category ? { kind: 'any_of', ids: [category] } : { kind: 'any' },
    source_kind: source === 'capture' || source === 'imported_image' ? source : null,
    application: { kind: 'any' }, date: { kind: 'any' }, has_ocr: null, has_applied_ai: null };
}
export function Results({ page, select }: { page: SearchPage | null; select: (hit: SearchHit) => void }) {
  return <ul className="gallery search-results">{page?.items.map(hit => <li key={hit.record_id}>
    <button data-record-id={hit.record_id} onClick={() => select(hit)}><strong>{hit.title}</strong><span>{hit.excerpt}</span>
      <small>{hit.category_label ?? 'Без категории'} · {hit.exact_match ? 'точный идентификатор' : 'текст'}</small></button>
  </li>)}</ul>;
}
export default function SearchPanel({ ready }: { ready: boolean }) {
  const [text, setText] = useState(''); const [category, setCategory] = useState(''); const [source, setSource] = useState('');
  const [catalog, setCatalog] = useState<CategoryCatalog | null>(null); const [page, setPage] = useState<SearchPage | null>(null);
  const [busy, setBusy] = useState(false); const [error, setError] = useState(''); const [refresh, setRefresh] = useState(0);
  const [selected, setSelected] = useState<SearchHit | null>(null); const [preview, setPreview] = useState('');
  const generation = useRef(new SearchGeneration()); const lane = useRef<LatestSearchLane | null>(null); const previewGeneration = useRef(0);
  const previewUrl = useRef('');
  function invalidate() {
    generation.current.next(); lane.current?.invalidate(); previewGeneration.current++;
    setError(''); setPage(null); setSelected(null); setPreview('');
    if (previewUrl.current) { URL.revokeObjectURL(previewUrl.current); previewUrl.current = ''; }
  }
  useEffect(() => {
    const current = new LatestSearchLane(request => invoke<SearchReply>('gui_search', { request }), (id, reply, reason) => {
      if (!generation.current.current(id)) return;
      if (reason) setError(typeof reason === 'string' ? reason : 'SEARCH_UNAVAILABLE');
      else if (reply?.kind === 'page') setPage(reply.value);
      setBusy(false);
    });
    lane.current = current;
    return () => { generation.current.next(); current.dispose(); previewGeneration.current++;
      if (previewUrl.current) { URL.revokeObjectURL(previewUrl.current); previewUrl.current = ''; }
    };
  }, []);
  useEffect(() => {
    if (!ready) return;
    let active = true;
    void invoke<{ kind: string; value: CategoryCatalog }>('gui_knowledge', { request: { action: 'categories' } })
      .then(r => { if (active) setCatalog(r.value); }).catch(() => { /* Search still works with Any; no invented categories. */ });
    return () => { active = false; };
  }, [ready, refresh]);
  useEffect(() => {
    const g = generation.current; const id = g.next();
    if (!ready) { invalidate(); setBusy(false); return; }
    const timer = setTimeout(() => {
      setBusy(true); setError('');
      const request: SearchRequest = { action: 'start', search_id: crypto.randomUUID(), text, filters: searchFilters(category, source), limit: 30, candidate_depth: 100 };
      lane.current?.enqueue(id, request);
    }, 180);
    return () => { clearTimeout(timer); g.next(); lane.current?.invalidate(); };
  }, [ready, text, category, source, refresh]);
  function nextPage() {
    if (!page?.next_cursor || busy) return;
    const id = generation.current.next(); setBusy(true); setError('');
    lane.current?.enqueue(id, { action: 'page', cursor: page.next_cursor });
  }
  async function select(hit: SearchHit) {
    const id = ++previewGeneration.current; setSelected(hit); setPreview('');
    if (previewUrl.current) { URL.revokeObjectURL(previewUrl.current); previewUrl.current = ''; }
    if (!canonicalAssetId(hit.asset_id)) { setError('PREVIEW_UNAVAILABLE'); return; }
    const assetId = hit.asset_id;
    try { const asset = await invoke<AssetPreview>('gui_asset_preview', { assetId: assetId });
      if (id !== previewGeneration.current) return;
      const url = URL.createObjectURL(new Blob([previewBytes(asset)], { type: 'image/webp' })); previewUrl.current = url; setPreview(url);
    } catch { if (id === previewGeneration.current) setError('PREVIEW_UNAVAILABLE'); }
  }
  return <section aria-label="Поиск" aria-busy={busy}>
    <h2>Поиск по библиотеке</h2><p>FTS5 / точные идентификаторы · работает при AI Off · слова объединяются OR, не семантика.</p>
    <label>Текст <input value={text} maxLength={1024} onChange={e => { invalidate(); setText(e.target.value); }} /></label>
    <label>Категория <select value={category} onChange={e => { invalidate(); setCategory(e.target.value); }}>
      <option value="">Любая</option><option value="uncategorized">Без категории</option>
      {catalog?.items.filter(c => c.state !== 'deleted').map(c => <option key={c.id} value={c.id}>{c.name}</option>)}
    </select></label>
    <label>Источник <select value={source} onChange={e => { invalidate(); setSource(e.target.value); }}>
      <option value="">Любой</option><option value="capture">Захват</option><option value="imported_image">Импорт</option>
    </select></label>
    <p>App/window/filename/OCR и даты captured/taken ещё не предоставлены accepted source; их фильтры явно Unsupported.</p>
    <button onClick={() => { invalidate(); setRefresh(n => n + 1); }} disabled={!ready}>Обновить поиск</button>
    {busy && <p role="status">Поиск…</p>}{error && <p role="alert">{error} — обновите поиск с новым snapshot.</p>}
    {page?.items.length === 0 && <p>Совпадений нет.</p>}
    <Results page={page} select={hit => { void select(hit); }} />
    {page?.window_truncated && <p>Показано фиксированное окно 100 результатов; CLI поддерживает новую глубину до 2000.</p>}
    {page?.next_cursor && <button onClick={() => { void nextPage(); }} disabled={busy}>Следующая страница поиска</button>}
    {selected && <div className="preview"><h3>{selected.title}</h3>{preview && <img src={preview} alt={selected.title} />}
      <KnowledgePanel recordId={selected.record_id} /></div>}
  </section>;
}
