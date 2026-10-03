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
      type: 'record_metadata';
      value: RecordMetadata;
    }
  | {
      type: 'records_page';
      value: LibraryRecordsPage;
    }
  | {
      type: 'import_report';
      value: ImportReport;
    }
  | {
      type: 'export_receipt';
      value: ExportReceipt;
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
export type ImportBatchId = string;
export type ImportItemId = string;
export type OriginId = string;
export type ImportState =
  | 'discovered'
  | 'processing'
  | 'held'
  | 'recoverable'
  | 'saved_new'
  | 'linked_existing'
  | 'skipped'
  | 'cancelled_unaccepted';
export type ImportBatchState =
  | 'running'
  | 'pausing'
  | 'paused'
  | 'cancelling'
  | 'cancelled'
  | 'needs_attention'
  | 'completed'
  | 'completed_with_errors';
export type ExportFormat = 'png' | 'jpeg' | 'webp' | 'avif';
export type ExportExistence = 'verified' | 'not_checked';
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
 * One bounded image transfer chunk. `library.asset.read` carries canonical bytes;
 * `library.asset.preview` carries a bounded immutable WebP derivative (which may
 * differ from the canonical). Digest/MIME/size describe that transfer, never a URL.
 * `data_hex` keeps the frame printable without a base64 dependency.
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
   * Decimal string of the complete transfer payload size.
   */
  total_size: string;
}
export interface RecordMetadata {
  record_id: RecordId;
  source_kind: string;
  source_reports: string[];
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
export interface ImportReport {
  active_work: boolean;
  batch_id: ImportBatchId;
  cancelled_unaccepted: number;
  control: string;
  directory_issues: number;
  discovered: number;
  discovery_abandoned: boolean;
  discovery_complete: boolean;
  discovery_limit: number;
  discovery_limit_hit: boolean;
  failed: number;
  held: number;
  in_flight: number;
  items: ImportItemReport[];
  linked_existing: number;
  next_ordinal?: number | null;
  retained_input: number;
  revision: Revision;
  saved_new: number;
  skipped: number;
  state: ImportBatchState;
}
export interface ImportItemReport {
  durable_input: boolean;
  item_id: ImportItemId;
  ordinal: number;
  receipt?: ImportReceipt | null;
  safe_code?: string | null;
  state: ImportState;
}
export interface ImportReceipt {
  canonical_mime: string;
  canonical_sha256: string;
  current_asset_id: AssetId;
  height: number;
  operation_id: OperationId;
  origin_id: OriginId;
  original_asset_id: AssetId;
  record_id: RecordId;
  relative_path: string;
  requested_profile_used: boolean;
  width: number;
}
export interface ExportReceipt {
  applied_parameters: ExportAppliedParameters;
  byte_size: string;
  encoding_profile_fingerprint: string;
  existence: ExportExistence;
  expires_at_epoch_seconds?: string | null;
  export_path: string;
  height: number;
  mime: string;
  operation_id: OperationId;
  partial: boolean;
  sha256: string;
  sidecar?: ExportSidecar | null;
  source_asset_id: AssetId;
  width: number;
}
export interface ExportAppliedParameters {
  format: ExportFormat;
  /**
   * @minItems 3
   * @maxItems 3
   */
  jpeg_background?: [number, number, number] | null;
  max_side?: number | null;
  private_metadata_stripped: boolean;
  quality?: number | null;
  resize_applied: boolean;
}
export interface ExportSidecar {
  byte_size: string;
  export_path: string;
  sha256: string;
}
export interface AppError {
  code: ErrorCode;
  retryable: boolean;
  safe_message: SafeMessage;
}
