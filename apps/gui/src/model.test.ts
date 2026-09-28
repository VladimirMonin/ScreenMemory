import { describe, expect, it } from 'vitest';
import {
  GUI_ENVELOPE_VERSION,
  absent,
  bridgeText,
  doctorFailed,
  guiBuild,
  notImplemented,
  stateText,
  statusFields,
  unfinishedChecks,
  type Diagnostics,
  type GuiState,
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
    'capture',
    'library_counts',
    'gallery',
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
    expect(listed).toContain('capture');
    expect(listed).toContain('library_counts');
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
