import { StrictMode } from 'react';
import { renderToStaticMarkup } from 'react-dom/server';
import { describe, expect, it, vi } from 'vitest';
import KnowledgePanel, { editRequest, pendingEdit, type CardDraft } from './KnowledgePanel';
import type { CardView, CategoryCatalog } from '../../../packages/protocol/src/generated/application-v1';
const invoke = vi.hoisted(() => vi.fn());
vi.mock('@tauri-apps/api/core', () => ({ invoke }));
const id = '00000000-0000-4000-8000-000000000001';
const card: CardView = { record_id: id, source_asset_id: id, record_revision: '4', content_revision: '2', overrides: { title: { op: 'inherit' }, description: { op: 'inherit' }, category_id: { op: 'inherit' } }, effective: { title: 'Technical', description: '', category_id: null, category_label: null, title_source: 'fallback', description_source: 'none', category_source: 'none' }, source: { source_version: 'manual_effective_v1', record_id: id, source_asset_id: id, title: 'Technical', description: '', category_id: null, category_label: null, required_read_scopes: ['read_metadata'] }, source_sha256: '0'.repeat(64), projection_state: 'handler_unavailable' };
const catalog: CategoryCatalog = { revision: '1', items: [] };
const draft: CardDraft = { title: 'private draft', description: '', categoryId: '', titleMode: 'keep', descriptionMode: 'set', categoryMode: 'set' };
describe('manual knowledge renderer', () => {
  it('StrictMode render neither edits nor requests AI, inventory, OCR or load', () => {
    invoke.mockClear();
    const html = renderToStaticMarkup(<StrictMode><KnowledgePanel recordId={id} /></StrictMode>);
    expect(html).toContain('Карточка и категории'); expect(html).toContain('при AI Off'); expect(html).toContain('Мои категории'); expect(invoke).not.toHaveBeenCalled();
  });
  it('empty user description and null category are Set, independent title stays Keep', () => {
    const req = editRequest(card, catalog, draft, id);
    expect(req).toMatchObject({ action: 'edit', expected_record_revision: '4', patch: { title: { op: 'keep' }, description: { op: 'set', value: '' }, category_id: { op: 'set', value: null } } });
    expect(editRequest(card, catalog, { ...draft, descriptionMode: 'inherit' }, id)).toMatchObject({ patch: { description: { op: 'inherit' } } });
  });
  it('lost-response retry is exact; conflict never changes draft or expected revision until explicit retry', () => {
    const initial = editRequest(card, catalog, draft, id);
    const fresh = { ...card, record_revision: '8' };
    expect(pendingEdit(initial, fresh, catalog, draft, crypto.randomUUID(), false)).toBe(initial);
    const retry = pendingEdit(initial, fresh, catalog, draft, '00000000-0000-4000-8000-000000000002', true);
    expect(retry).toMatchObject({ expected_record_revision: '8', operation_id: '00000000-0000-4000-8000-000000000002', patch: { description: { op: 'set', value: '' } } });
    expect(draft.title).toBe('private draft'); expect(card.record_revision).toBe('4');
  });
});
