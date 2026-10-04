import { StrictMode } from 'react';
import { renderToStaticMarkup } from 'react-dom/server';
import { describe, expect, it, vi } from 'vitest';
import SearchPanel, { Results, SearchGeneration, searchFilters } from './SearchPanel';
import type { SearchPage } from '../../../packages/protocol/src/generated/application-v1';
const invoke = vi.hoisted(() => vi.fn());
vi.mock('@tauri-apps/api/core', () => ({ invoke }));
describe('production search renderer', () => {
  it('mount renders explicit FTS5/Off/unsupported without model or mutable side effects', () => {
    invoke.mockClear(); const html = renderToStaticMarkup(<StrictMode><SearchPanel ready /></StrictMode>);
    expect(html).toContain('FTS5'); expect(html).toContain('AI Off'); expect(html).toContain('Unsupported'); expect(invoke).not.toHaveBeenCalled();
  });
  it('actual generation guard rejects old request even if its answer is newer by time', async () => {
    const g = new SearchGeneration(); const old = g.next(); const latest = g.next();
    const shown: string[] = []; await Promise.resolve().then(() => { if (g.current(latest)) shown.push('new'); });
    await Promise.resolve().then(() => { if (g.current(old)) shown.push('old'); }); expect(shown).toEqual(['new']);
    g.next(); expect(g.current(latest)).toBe(false);
  });
  it('category IDs remain stable/OR-group and source filter is explicit', () => {
    expect(searchFilters('00000000-0000-4000-8000-000000000001', 'imported_image')).toMatchObject({ category: { kind: 'any_of', ids: ['00000000-0000-4000-8000-000000000001'] }, source_kind: 'imported_image' });
    expect(searchFilters('uncategorized', '')).toMatchObject({ category: { kind: 'uncategorized' }, source_kind: null });
  });
  it('results show same title/excerpt/category and escape markup, no innerHTML', () => {
    const page: SearchPage = { search_id: 's', projection_epoch: '2', candidate_depth: 100, window_truncated: false, next_cursor: null,
      items: [{ record_id: 'r', asset_id: 'a', content_revision: '2', title: 'Q8_0 <script>', excerpt: '<b>plain text</b>', category_id: 'c', category_label: 'Development', exact_match: true, lexical_match: true, exact_terms_truncated: false, position: 1 }] };
    const html = renderToStaticMarkup(<Results page={page} select={() => undefined} />);
    expect(html).toContain('Q8_0 &lt;script&gt;'); expect(html).toContain('&lt;b&gt;plain text&lt;/b&gt;'); expect(html).toContain('Development'); expect(html).not.toContain('<script>');
  });
});
