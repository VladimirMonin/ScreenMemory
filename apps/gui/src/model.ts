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
