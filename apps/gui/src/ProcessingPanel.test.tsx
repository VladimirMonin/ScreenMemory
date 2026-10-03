import { StrictMode } from 'react';
import { renderToStaticMarkup } from 'react-dom/server';
import { describe, expect, it, vi } from 'vitest';
import ProcessingPanel, { stableProbePlan } from './ProcessingPanel';
import type { ProcessingRequest } from '../../../packages/protocol/src/generated/application-methods-v1';
const calls = vi.hoisted(() => vi.fn());
vi.mock('@tauri-apps/api/core', () => ({ invoke: calls }));
const id = '00000000-0000-4000-8000-000000000001';
const op = '00000000-0000-4000-8000-000000000002';
type Plan = Extract<ProcessingRequest, { action: 'plan' }>;
const request: Plan = { action: 'plan', operation_id: op, expected_policy_revision: '1', records: [id], kind: 'technical_owner_self_test', profile_id: id };
describe('S04 renderer has no runtime owner', () => {
  it('renders useful lazy controls under StrictMode without inventory/load/inference', () => {
    calls.mockClear();
    for (let n = 0; n < 2; n += 1) {
      const html = renderToStaticMarkup(<StrictMode><ProcessingPanel recordId={id} libraryId={id} /></StrictMode>);
      expect(html).toContain('Обработка · настройки, модели и очередь');
      expect(html).toContain('Off по умолчанию');
      expect(html).toContain('blocked:handler_unavailable');
      expect(html).toContain('Обновить настройки и очередь');
    }
    expect(calls).not.toHaveBeenCalled();
  });
  it('lost response and reopen retain the exact operation instead of second load intent', () => {
    const candidate = { ...request, operation_id: crypto.randomUUID() };
    expect(stableProbePlan(JSON.stringify(request), candidate)).toEqual(request);
    expect(stableProbePlan(JSON.stringify(request), { ...candidate, expected_policy_revision: '2' })).toEqual({ ...candidate, expected_policy_revision: '2' });
  });
  it('invalid/injected local receipt cannot become the candidate', () => {
    for (const raw of ['not-json', 'null', JSON.stringify({ ...request, token: 'injected' }), JSON.stringify({ ...request, operation_id: 'not-id' }), JSON.stringify({ ...request, kind: 'analyze' })]) {
      expect(stableProbePlan(raw, request)).toBe(request);
    }
  });
});
