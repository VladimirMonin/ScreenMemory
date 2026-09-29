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
    }
  | {
      type: 'capture';
      value: CaptureOutcome;
    }
  | {
      type: 'asset_chunk';
      value: LibraryAssetChunk;
    }
  | {
      type: 'records_page';
      value: LibraryRecordsPage;
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
  | (
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
      | 'WAIT_TIMEOUT'
      | 'CAPTURE_BACKPRESSURE'
      | 'CAPTURE_TARGET_LOST'
      | 'CAPTURE_TOPOLOGY_CHANGED'
      | 'CAPTURE_FOCUS_UNAVAILABLE'
      | 'CAPTURE_UNAVAILABLE'
      | 'CAPTURE_GDI_FAILED'
      | 'IMAGE_LIMIT_EXCEEDED'
      | 'CANONICAL_SIZE_UNSUPPORTED'
      | 'ASSET_INTEGRITY_FAILED'
      | 'ASSET_NOT_FOUND'
      | 'STORAGE_FULL'
    )
  | 'CAPTURE_BUSY'
  | 'CODEC_FAILED'
  | 'IMAGE_INPUT_INVALID';
export type ReportId = string;
export type Revision = string;
export type ConsentId = string;
export type UtcTime = string;
/**
 * S02 §5.2: terminal outcome of one `capture.command`.
 */
export type CaptureOutcome =
  | {
      receipt: CaptureReceipt;
      status: 'saved';
      [k: string]: unknown;
    }
  | {
      operation_id: OperationId;
      reason: CancelReason;
      status: 'cancelled';
      [k: string]: unknown;
    };
export type AssetId = string;
export type OperationId = string;
export type RecordId = string;
/**
 * S02 §5.2: why a capture ended without an accepted frame.
 */
export type CancelReason = 'escape' | 'empty_selection' | 'explicit_request' | 'client_disconnected';
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
/**
 * S02 §5.2/§11.5: receipt of one committed capture. No pixels and no absolute path.
 */
export interface CaptureReceipt {
  asset_id: AssetId;
  /**
   * Decimal string: a byte count must not round through a JavaScript number.
   */
  byte_size: string;
  canonical_mime: string;
  canonical_sha256: string;
  height: number;
  operation_id: OperationId;
  record_id: RecordId;
  /**
   * Library-relative canonical path (`assets/...`), never an absolute path.
   */
  relative_path: string;
  warnings: string[];
  width: number;
}
/**
 * S02 §10.1: one bounded chunk of a canonical asset. `data_hex` keeps the frame a
 * printable JSON document without a base64 dependency, and `sha256` is the whole-file
 * digest so a truncated or substituted file is refused instead of exported.
 */
export interface LibraryAssetChunk {
  asset_id: AssetId;
  data_hex: string;
  eof: boolean;
  mime: string;
  next_offset: number;
  offset: number;
  sha256: string;
  /**
   * Decimal string of the whole canonical file size.
   */
  total_size: string;
}
/**
 * S02 §10.3: one bounded page of committed records; `next_cursor` is opaque.
 */
export interface LibraryRecordsPage {
  items: LibraryRecordEntry[];
  next_cursor?: string | null;
}
/**
 * S02 §10.3: one entry in a library records page; asset paths are library-relative.
 */
export interface LibraryRecordEntry {
  byte_size: string;
  created_at: string;
  height: number;
  id: RecordId;
  preview_asset_id?: AssetId | null;
  target: string;
  title: string;
  width: number;
}
export interface AppError {
  code: ErrorCode;
  retryable: boolean;
  safe_message: SafeMessage;
}
