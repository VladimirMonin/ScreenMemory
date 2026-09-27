/* Generated from sm-protocol JSON Schema; do not edit. Runtime validation uses Ajv. */

export type ApplicationFrame = RequestEnvelope | Response;
export type LibraryId = string;
export type MethodName = string;
export type RequestId = string;
export type Response =
  | {
      kind: 'result';
      request_id: RequestId;
      result: ApplicationResult;
    }
  | {
      error: AppError;
      kind: 'error';
      request_id: RequestId;
    };
export type ApplicationResult =
  | {
      type: 'status';
      value: HostStatus;
    }
  | {
      type: 'doctor';
      value: DoctorReport;
    }
  | {
      type: 'probe_runtime';
      value: RuntimeProbeReport;
    }
  | {
      type: 'ai_policy';
      value: AiPolicyView;
    }
  | {
      type: 'probe_consent';
      value: ProbeConsentReceipt;
    }
  | {
      type: 'shutdown';
      value: ShutdownReceipt;
    };
export type BridgeState = 'stopped' | 'starting' | 'ready' | 'degraded' | 'unavailable';
export type BuildId = string;
export type HostInstanceId = string;
export type HostState = 'starting' | 'ready' | 'degraded' | 'stopping';
export type CheckId = string;
export type EvidenceId = string;
export type CheckOutcome = 'PASSED' | 'FAILED' | 'BLOCKED' | 'NOT_RUN' | 'UNSUPPORTED';
/**
 * Fixed public registry. Never expose provider exception strings in `safe_message`.
 */
export type ErrorCode =
  | 'METHOD_NOT_SUPPORTED'
  | 'INVALID_REQUEST'
  | 'FRAME_TOO_LARGE'
  | 'PROTOCOL_TRUNCATED_FRAME'
  | 'PERMISSION_DENIED'
  | 'LIBRARY_MISMATCH'
  | 'PROTOCOL_MISMATCH'
  | 'HOST_ALREADY_RUNNING'
  | 'SCHEMA_INCOMPATIBLE'
  | 'CREDENTIAL_STORE_UNAVAILABLE'
  | 'RUNTIME_UNAVAILABLE'
  | 'MODEL_NOT_INSTALLED'
  | 'AI_DISABLED'
  | 'MODEL_CONFIG_UNSUPPORTED'
  | 'BRIDGE_UNAVAILABLE'
  | 'WAIT_TIMEOUT';
export type ReportId = string;
export type Revision = string;
export type ConsentId = string;
export type UtcTime = string;
export type SafeMessage = string;

/**
 * Decoded after the authorized handshake; `params` never carries a trusted context.
 */
export interface RequestEnvelope {
  library_id: LibraryId;
  method: MethodName;
  params: unknown;
  protocol: ProtocolVersion;
  request_id: RequestId;
  timeout_ms: number;
}
export interface ProtocolVersion {
  major: number;
  minor: number;
}
export interface HostStatus {
  ai_enabled: boolean;
  available_methods: MethodName[];
  bridge_state: BridgeState;
  build_id: BuildId;
  host_instance_id: HostInstanceId;
  library_id: LibraryId;
  protocol: ProtocolVersion;
  schema_version: number;
  state: HostState;
}
export interface DoctorReport {
  checks: CheckResult[];
  host: HostStatus;
  report_id: ReportId;
}
export interface CheckResult {
  check_id: CheckId;
  duration_ms?: number;
  evidence_id?: EvidenceId;
  outcome: CheckOutcome;
  safe_code?: ErrorCode;
}
export interface RuntimeProbeReport {
  checks: CheckResult[];
  compatibility_evidence_id: EvidenceId;
  probe_id: ReportId;
}
export interface AiPolicyView {
  enabled: boolean;
  revision: Revision;
}
export interface ProbeConsentReceipt {
  consent_id: ConsentId;
  expires_at: UtcTime;
  policy_revision: Revision;
}
export interface ShutdownReceipt {
  host_instance_id: HostInstanceId;
  state: HostState;
}
export interface AppError {
  code: ErrorCode;
  retryable: boolean;
  safe_message: SafeMessage;
}
