import { describe, expect, it } from 'vitest';
import {
  GUI_ENVELOPE_VERSION,
  absent,
  bridgeText,
  captureFields,
  captureModeText,
  captureModes,
  captureStateText,
  canonicalAssetId,
  previewBytes,
  MAX_PREVIEW_BYTES,
  doctorFailed,
  guiBuild,
  notImplemented,
  stateText,
  statusFields,
  unfinishedChecks,
  type CaptureAnswer,
  type Diagnostics,
  type GuiState,
  type AssetPreview,
} from './model';

const readyHost: Diagnostics['host'] = {
  ai_enabled: false,
  available_methods: [],
  bridge_state: 'stopped',
  build_id: 'synthetic',
  host_instance_id: '01900000-0000-7000-8000-000000000002',
  library_id: '01900000-0000-7000-8000-000000000001',
  protocol: { major: 1, minor: 0 },
  schema_version: 1,
  state: 'ready',
};

const ready: Diagnostics = {
  schema_version: GUI_ENVELOPE_VERSION,
  state: 'ready',
  host: readyHost,
  doctor: null,
  runtime_available: false,
  safe_code: null,
  retryable: false,
  bounded_ms: 5000,
  report_available: true,
  not_implemented: [
    'search',
    'drop_zone',
    'model_settings',
    'bridge_runtime',
  ],
};

describe('S01 diagnostic state mapping', () => {
  it('covers every documented state of S01 §10.1 without invented counts', () => {
    const states: GuiState[] = [
      'host_not_running',
      'connecting',
      'ready',
      'runtime_unavailable',
      'protocol_mismatch',
      'root_credential_error',
    ];
    expect(Object.keys(stateText).sort()).toEqual([...states].sort());
    for (const state of states) {
      expect(stateText[state]).toBeTruthy();
      expect(stateText[state]).not.toMatch(/0 изображений|проиндексировано|найдено/);
    }
  });

  it('shows unknown values before any status instead of zero images', () => {
    expect(statusFields(null).slice(1).every(([, value]) => value === absent)).toBe(true);
  });

  it('renders real host IDs, versions, AI policy and bridge state without substituting values', () => {
    expect(Object.fromEntries(statusFields(ready))).toMatchObject({
      'Host instance ID': readyHost.host_instance_id,
      'Library ID': readyHost.library_id,
      'Host build': 'synthetic',
      'Host protocol': '1.0',
      Schema: '1',
      'AI policy': 'выключена',
      'AI runtime': 'недоступен',
      'Bridge state': 'остановлен',
    });
  });

  it('reports the enabled AI policy with an unusable bridge as runtime unavailable', () => {
    const fields = Object.fromEntries(
      statusFields({ ...ready, runtime_available: false, host: { ...ready.host!, ai_enabled: true } }),
    );
    expect(fields['AI policy']).toBe('включена');
    expect(fields['AI runtime']).toBe('недоступен');
    const available = Object.fromEntries(
      statusFields({
        ...ready,
        runtime_available: true,
        host: { ...ready.host!, ai_enabled: true, bridge_state: 'ready' },
      }),
    );
    expect(available['AI runtime']).toBe('доступен');
    expect(available['Bridge state']).toBe('готов');
  });

  it('maps every wire bridge state and never falls back to a raw value', () => {
    const states = Object.keys(bridgeText) as (keyof typeof bridgeText)[];
    expect(states.sort()).toEqual(['degraded', 'ready', 'starting', 'stopped', 'unavailable']);
    for (const state of states) {
      expect(bridgeText[state]).toBeTruthy();
      const fields = Object.fromEntries(
        statusFields({ ...ready, host: { ...ready.host!, bridge_state: state } }),
      );
      expect(fields['Bridge state']).toBe(bridgeText[state]);
    }
  });

  it('separates a failed mandatory check from an unfinished one', () => {
    const failed: Diagnostics = {
      ...ready,
      doctor: {
        host: readyHost!,
        report_id: '01900000-0000-7000-8000-000000000003',
        checks: [
          { check_id: 'storage.schema', outcome: 'FAILED' },
          { check_id: 'runtime.lmstudio', outcome: 'NOT_RUN' },
          { check_id: 'index.fts_vec', outcome: 'UNSUPPORTED' },
          { check_id: 'codec.webp_avif', outcome: 'BLOCKED' },
        ],
      },
    };
    expect(doctorFailed(failed)).toBe(true);
    expect(unfinishedChecks(failed).map((check) => check.check_id)).toEqual([
      'runtime.lmstudio',
      'index.fts_vec',
      'codec.webp_avif',
    ]);
    const clean: Diagnostics = {
      ...failed,
      doctor: {
        ...failed.doctor!,
        checks: [
          { check_id: 'storage.schema', outcome: 'PASSED' },
          { check_id: 'runtime.lmstudio', outcome: 'NOT_RUN' },
        ],
      },
    };
    expect(doctorFailed(clean)).toBe(false);
    expect(unfinishedChecks(clean).map((check) => check.check_id)).toEqual(['runtime.lmstudio']);
  });

  it('shows the owner list of unfinished capabilities and never invents one', () => {
    expect(notImplemented(null)).toEqual([]);
    const listed = notImplemented(ready);
    expect(listed).toContain('search');
    expect(listed).not.toContain('gallery');
    expect(listed).not.toContain('capture');
    expect(listed.every((name) => !name.includes('images'))).toBe(true);
  });
});

describe('S01 diagnostic envelope shape', () => {
  it('keeps the envelope version of this slice and its build identity', () => {
    expect(GUI_ENVELOPE_VERSION).toBe(1);
    expect(ready.schema_version).toBe(GUI_ENVELOPE_VERSION);
    expect(guiBuild.version).toBe('0.1.0');
    expect(guiBuild.protocol).toEqual({ major: 1, minor: 0 });
  });

  it('never exposes an image count, a path or a model field', () => {
    const keys = Object.keys(ready);
    expect(keys.some((key) => /count|path|model|image/i.test(key))).toBe(false);
  });
});

describe('S02 bounded gallery preview decoder', () => {
  const image: AssetPreview = {
    schema_version: 1, mime: 'image/webp', width: 0, height: 0,
    data_hex: '52494646', safe_code: null, retryable: false,
  };
  it('accepts only canonical v4 asset identities', () => {
    expect(canonicalAssetId('01900000-0000-4000-8000-000000000001')).toBe(true);
    expect(canonicalAssetId('01900000-0000-7000-8000-000000000001')).toBe(false);
    expect(canonicalAssetId('../file.webp')).toBe(false);
    expect(canonicalAssetId('01900000-0000-4000-8000-00000000000a'.toUpperCase())).toBe(false);
  });
  it('decodes only bounded WebP hex and rejects oversized/malformed/unavailable', () => {
    expect([...previewBytes(image)]).toEqual([82, 73, 70, 70]);
    expect(() => previewBytes({ ...image, mime: 'image/png' })).toThrow('PREVIEW_UNAVAILABLE');
    expect(() => previewBytes({ ...image, safe_code: 'PREVIEW_TOO_LARGE' })).toThrow('PREVIEW_TOO_LARGE');
    expect(() => previewBytes({ ...image, data_hex: '0' })).toThrow('PREVIEW_UNAVAILABLE');
    expect(() => previewBytes({ ...image, data_hex: 'zz' })).toThrow('PREVIEW_UNAVAILABLE');
    expect(() => previewBytes({ ...image, data_hex: '00'.repeat(MAX_PREVIEW_BYTES + 1) })).toThrow('PREVIEW_UNAVAILABLE');
  });
});

describe('S02 capture answer of the diagnostic window', () => {
  const saved: CaptureAnswer = {
    schema_version: GUI_ENVELOPE_VERSION,
    state: 'saved',
    operation_id: '01900000-0000-7000-8000-000000000201',
    record_id: '01900000-0000-7000-8000-000000000001',
    asset_id: '01900000-0000-7000-8000-000000000101',
    width: 1000,
    height: 600,
    byte_size: '48271',
    relative_path: 'assets/2026-09/01900000-0000-7000-8000-000000000001/original.webp',
    reason: null,
    safe_code: null,
    retryable: false,
    warnings: [],
  };

  it('shows the committed identities and geometry without inventing a count', () => {
    const fields = Object.fromEntries(captureFields(saved));
    expect(fields['Захват']).toBe(captureStateText.saved);
    expect(fields['Record ID']).toBe(saved.record_id);
    expect(fields['Asset ID']).toBe(saved.asset_id);
    expect(fields['Размер']).toBe('1000 × 600');
    expect(fields['Байт']).toBe('48271');
    expect(fields['Канонический путь']).toBe(saved.relative_path);
    expect(Object.keys(fields).some((key) => /count|image/i.test(key))).toBe(false);
  });

  it('separates a cancellation and a refusal from a saved capture', () => {
    const cancelled: CaptureAnswer = {
      ...saved,
      state: 'cancelled',
      record_id: null,
      asset_id: null,
      width: null,
      height: null,
      byte_size: null,
      relative_path: null,
      reason: 'escape',
    };
    expect(Object.fromEntries(captureFields(cancelled))['Захват']).toBe(captureStateText.cancelled);
    expect(Object.fromEntries(captureFields(cancelled))['Record ID']).toBe(absent);
    const refused: CaptureAnswer = {
      ...cancelled,
      state: 'refused',
      reason: null,
      safe_code: 'CAPTURE_FOCUS_UNAVAILABLE',
    };
    const refusedFields = Object.fromEntries(captureFields(refused));
    expect(refusedFields['Захват']).toBe(captureStateText.refused);
    expect(refusedFields['Безопасный код']).toBe('CAPTURE_FOCUS_UNAVAILABLE');
    expect(captureFields(null)).toEqual([['Захват', absent]]);
  });

  it('keeps the requested mode vocabulary closed', () => {
    expect([...captureModes]).toEqual(['region', 'window', 'monitor', 'desktop']);
    for (const mode of captureModes) {
      expect(captureModeText[mode]).toBeTruthy();
    }
    expect(Object.keys(captureModeText).sort()).toEqual([...captureModes].sort());
  });
});
