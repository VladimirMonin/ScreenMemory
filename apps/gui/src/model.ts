import type {
  BridgeState,
  CheckResult,
  DoctorReport,
  ErrorCode,
  HostStatus,
} from '../../../packages/protocol/src/generated/application-v1';

/**
 * Version of this window's own diagnostics envelope. It mirrors
 * `GUI_ENVELOPE_VERSION` in `crates/gui/src/model.rs`; `model.test.ts` reads the Rust
 * constant and fails on drift, so the two numbers can never silently diverge.
 */
export const GUI_ENVELOPE_VERSION = 1;

/**
 * Identity of this frontend build. `version` mirrors `apps/gui/package.json` and
 * `protocol` mirrors `APPLICATION_VERSION` of `crates/protocol/src/application.rs`; both
 * are cross-checked by `model.test.ts` instead of being trusted here (S01 §10.1 asks for
 * build/protocol versions, and §5.3 keeps the protocol version a single owned constant).
 */
export const guiBuild = {
  version: '0.1.0',
  protocol: { major: 1, minor: 0 },
} as const;

/** The six documented window states of S01 §10.1 plus the bounded `connecting` attempt. */
export type GuiState =
  | 'host_not_running'
  | 'connecting'
  | 'ready'
  | 'runtime_unavailable'
  | 'protocol_mismatch'
  | 'root_credential_error';

/** One answer of a `sm-gui` command handler; the same fields the Rust envelope carries. */
export interface Diagnostics {
  schema_version: number;
  state: GuiState;
  host: HostStatus | null;
  doctor: DoctorReport | null;
  runtime_available: boolean;
  safe_code: ErrorCode | null;
  retryable: boolean;
  bounded_ms: number;
  report_available: boolean;
  not_implemented: string[];
}

export const stateText: Record<GuiState, string> = {
  host_not_running: 'Host не запущен или недоступен. Можно подключиться к настроенному host.',
  connecting: 'Подключение… Ожидание ограничено; вызовов моделей нет.',
  ready: 'Основа доступна.',
  runtime_unavailable: 'Основа доступна, AI runtime недоступен.',
  protocol_mismatch: 'Версии протокола несовместимы. Нужна совместимая сборка.',
  root_credential_error: 'Ошибка root или учётных данных. Другая библиотека не создаётся.',
};

/** Total mapping of the wire bridge states; an unmapped value would fail the typecheck. */
export const bridgeText: Record<BridgeState, string> = {
  stopped: 'остановлен',
  starting: 'запускается',
  ready: 'готов',
  degraded: 'работает с ограничениями',
  unavailable: 'недоступен',
};

export const absent = 'нет подтверждённых данных';

/** The only fields the window shows; unknown values stay explicitly absent, never zero. */
export function statusFields(report: Diagnostics | null): [string, string][] {
  const host = report?.host;
  return [
    [
      'GUI build / application protocol',
      `${guiBuild.version} / ${guiBuild.protocol.major}.${guiBuild.protocol.minor}`,
    ],
    ['Host build', host?.build_id ?? absent],
    ['Host protocol', host ? `${host.protocol.major}.${host.protocol.minor}` : absent],
    ['Host instance ID', host?.host_instance_id ?? absent],
    ['Library ID', host?.library_id ?? absent],
    ['Schema', host ? String(host.schema_version) : absent],
    ['AI policy', host ? (host.ai_enabled ? 'включена' : 'выключена') : absent],
    ['AI runtime', report ? (report.runtime_available ? 'доступен' : 'недоступен') : absent],
    ['Bridge state', host ? bridgeText[host.bridge_state] : absent],
  ];
}

/** `doctor` reports a real failure only for an actual `FAILED` mandatory check. */
export function doctorFailed(report: Diagnostics | null): boolean {
  return report?.doctor?.checks.some((check) => check.outcome === 'FAILED') ?? false;
}

/**
 * Checks that were not proven: `NOT_RUN`, `BLOCKED` and `UNSUPPORTED` are shown as such and
 * are never presented as passed (the `doctor` command itself stays successful, S01 §10.3).
 * An actual `FAILED` check is reported separately by `doctorFailed` instead of being
 * mixed into this list.
 */
export function unfinishedChecks(report: Diagnostics | null): CheckResult[] {
  return (report?.doctor?.checks ?? []).filter((check) =>
    check.outcome === 'NOT_RUN' || check.outcome === 'BLOCKED' || check.outcome === 'UNSUPPORTED',
  );
}

/**
 * The capabilities the Rust owner of this slice lists as absent. Without an answer from
 * the handler the window shows nothing instead of inventing a list or an empty result.
 */
export function notImplemented(report: Diagnostics | null): string[] {
  return report?.not_implemented ?? [];
}

/**
 * Terminal state of one capture answer; it mirrors `GuiCaptureState` in
 * `crates/gui/src/model.rs`. A cancellation and a refusal are their own states, so the
 * window can never show an empty gallery entry as a successful capture.
 */
export type CaptureState = 'saved' | 'cancelled' | 'refused';

/** One capture answer: identities and geometry only, never pixels or paths of the host. */
export interface CaptureAnswer {
  schema_version: number;
  state: CaptureState;
  operation_id: string | null;
  record_id: string | null;
  asset_id: string | null;
  width: number | null;
  height: number | null;
  byte_size: string | null;
  relative_path: string | null;
  reason: string | null;
  safe_code: string | null;
  retryable: boolean;
  warnings: string[];
}

/** The modes the window may request; the same closed vocabulary the host accepts. */
export const captureModes = ['region', 'window', 'monitor', 'desktop'] as const;
export type CaptureMode = (typeof captureModes)[number];

/**
 * One *argument-free* handler per mode: the WebView never sends a parameter, and the
 * registered handler set is exactly this closed table (mirrors `crates/gui/src/shell.rs`).
 */
export const captureCommand: Record<CaptureMode, string> = {
  region: 'gui_capture_region',
  window: 'gui_capture_window',
  monitor: 'gui_capture_monitor',
  desktop: 'gui_capture_desktop',
};

export interface LibraryRecord {
  id: string;
  created_at: string;
  title: string;
  target: string;
  width: number;
  height: number;
  byte_size: string;
  preview_asset_id: string | null;
}

export interface LibraryPage {
  schema_version: number;
  items: LibraryRecord[];
  next_cursor: string | null;
  safe_code: string | null;
  retryable: boolean;
}

export interface AssetPreview {
  schema_version: number;
  mime: string;
  width: number;
  height: number;
  data_hex: string;
  safe_code: string | null;
  retryable: boolean;
}

export const MAX_PREVIEW_BYTES = 8 * 1024 * 1024;
export const canonicalAssetId = (id: string): boolean =>
  /^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/.test(id);

/** The native host bounds the derivative separately from the retained canonical size. */
export function mergeLibraryPage(previous: LibraryPage | null, page: LibraryPage, append: boolean): LibraryPage {
  return append && previous ? { ...page, items: [...previous.items, ...page.items.filter((item) => !previous.items.some((old) => old.id === item.id))] } : page;
}
export function canAutoPreview(item: LibraryRecord): boolean {
  return item.preview_asset_id !== null && canonicalAssetId(item.preview_asset_id)
    && /^[1-9][0-9]{0,8}$/.test(item.byte_size)
    && Number(item.byte_size) <= 128 * 1024 * 1024;
}

/** Decode only the validated, size-capped WebP response; never feed raw hex to an URL. */
export function previewBytes(preview: AssetPreview): Uint8Array<ArrayBuffer> {
  if (preview.safe_code) throw new Error(preview.safe_code);
  const hex = preview.data_hex;
  if (preview.schema_version !== GUI_ENVELOPE_VERSION || preview.mime !== 'image/webp'
      || hex.length === 0 || hex.length > MAX_PREVIEW_BYTES * 2
      || hex.length % 2 !== 0 || !/^(?:[0-9a-f]{2})+$/.test(hex)) {
    throw new Error('PREVIEW_UNAVAILABLE');
  }
  const bytes = new Uint8Array(hex.length / 2);
  for (let i = 0; i < bytes.length; i += 1) bytes[i] = parseInt(hex.slice(i * 2, i * 2 + 2), 16);
  return bytes;
}

export const captureStateText: Record<CaptureState, string> = {
  saved: 'Снимок сохранён в библиотеку.',
  cancelled: 'Захват отменён; изображение не создавалось.',
  refused: 'Host отказал в захвате.',
};

export const captureModeText: Record<CaptureMode, string> = {
  region: 'Область',
  window: 'Активное окно',
  monitor: 'Монитор под курсором',
  desktop: 'Все мониторы',
};

/** Fields the window shows for a capture answer; unknown values stay explicitly absent. */
export function captureFields(answer: CaptureAnswer | null): [string, string][] {
  if (!answer) return [['Захват', absent]];
  const rows: [string, string][] = [
    ['Захват', captureStateText[answer.state]],
    ['Operation ID', answer.operation_id ?? absent],
    ['Record ID', answer.record_id ?? absent],
    ['Asset ID', answer.asset_id ?? absent],
    [
      'Размер',
      answer.width !== null && answer.height !== null ? `${answer.width} × ${answer.height}` : absent,
    ],
    ['Байт', answer.byte_size ?? absent],
    ['Канонический путь', answer.relative_path ?? absent],
    ['Причина отмены', answer.reason ?? absent],
  ];
  if (answer.safe_code) rows.push(['Безопасный код', answer.safe_code]);
  return rows;
}
