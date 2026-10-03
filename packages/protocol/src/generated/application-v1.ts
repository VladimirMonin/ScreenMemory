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
    }
  | {
      type: 'processing';
      value: ProcessingReply;
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
      | 'REVISION_CONFLICT'
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
export type ProcessingReply =
  | {
      kind: 'policy';
      value: PolicyView;
    }
  | {
      kind: 'overview';
      value: ProcessingOverview;
    }
  | {
      kind: 'inventory';
      value: InventoryView;
    }
  | {
      kind: 'profile';
      value: ProfileView;
    }
  | {
      kind: 'batch';
      value: BatchView;
    }
  | {
      kind: 'job';
      value: JobView;
    }
  | {
      kind: 'execution';
      value: ExecutionReport;
    };
export type LoadPermission = 'allow' | 'forbid';
export type ProcessingMode = 'automatic' | 'manual' | 'off';
export type QueryLoadPermission = 'ask' | 'allow' | 'forbid';
export type HostRetention =
  | {
      kind: 'no_host_eviction';
    }
  | {
      kind: 'idle';
      seconds: number;
    }
  | {
      kind: 'after_batch';
    }
  | {
      kind: 'keep_loaded';
    };
export type ProcessingBatchId = string;
export type ProcessingJobId = string;
export type WorkKind = 'analyze' | 'technical_owner_self_test' | 'technical_owner_background_self_test';
export type ModelProfileId = string;
export type CacheType = 'f16' | 'f32' | 'q8' | 'q4' | 'q5';
export type ReasoningMode = 'backend_default' | 'off' | 'budget100';
export type BlockReason =
  | 'handler_unavailable'
  | 'runtime_unconfigured'
  | 'ai_disabled'
  | 'interrupted'
  | 'backend_uncertain'
  | 'cancel_requested'
  | 'profile_retired'
  | 'runtime_refused'
  | 'lease_expired';
export type JobState = 'pending' | 'running' | 'blocked' | 'failed' | 'cancelled' | 'superseded' | 'succeeded';
export type BatchControl = 'pause' | 'resume' | 'cancel';
export type ProcessingAttemptId = string;
export type AttemptState = 'claimed' | 'dispatched' | 'settled' | 'uncertain' | 'interrupted';
export type RuntimeCallId = string;
export type RuntimeCallKind = 'load' | 'inference';
export type LoadConfigurationStatus = 'reported_applied' | 'mismatch' | 'unknown';
export type RuntimeCallState = 'planned' | 'settled' | 'uncertain';
export type ModelConsumerLeaseId = string;
export type ModelConsumerLeaseState = 'active' | 'released' | 'abandoned' | 'uncertain';
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
export interface PolicyView {
  authorization_epoch: Revision;
  revision: Revision;
  value: ProcessingPolicy;
}
export interface ProcessingPolicy {
  background_loading: LoadPermission;
  mode: ProcessingMode;
  query_loading: QueryLoadPermission;
  queue_paused: boolean;
  retention: HostRetention;
}
export interface ProcessingOverview {
  analyze_handler_available: boolean;
  bridge_state: BridgeState;
  /**
   * @maxItems 32
   */
  jobs: QueuedJob[];
  model_autostart: boolean;
  native_ttl_verification: string;
  next_before?: ProcessingJobId | null;
  physical_unload_supported: boolean;
  policy: PolicyView;
  /**
   * @maxItems 32
   */
  profiles: ProfileView[];
  runtime_configured: boolean;
  supported_reasoning: ReasoningMode[];
  supported_retention: HostRetention[];
}
export interface QueuedJob {
  batch_id: ProcessingBatchId;
  batch_revision: Revision;
  job: JobView;
}
export interface JobView {
  asset_id: AssetId;
  asset_sha256: string;
  attempt_count: number;
  authorization_epoch: Revision;
  id: ProcessingJobId;
  kind: WorkKind;
  policy_revision: Revision;
  probe_result?: TechnicalProbeResult | null;
  profile?: ProfileView | null;
  reason?: BlockReason | null;
  record_id: RecordId;
  revision: Revision;
  state: JobState;
}
export interface TechnicalProbeResult {
  generated_tokens?: number | null;
  image_operation_id: OperationId;
  reasoning_tokens?: number | null;
  stop_reason: string;
  validated: boolean;
}
export interface ProfileView {
  fingerprint: string;
  revision: Revision;
  value: ModelProfile;
}
export interface ModelProfile {
  context_length: number;
  eval_batch_size: number;
  flash_attention: boolean;
  id: ModelProfileId;
  k_cache: CacheType;
  max_generation_tokens: number;
  model_key: string;
  native_ttl_seconds?: number | null;
  offload_kv_cache_to_gpu: boolean;
  reasoning: ReasoningMode;
  temperature: number;
  v_cache: CacheType;
}
export interface InventoryView {
  /**
   * @maxItems 512
   */
  downloaded: InventoryModel[];
  /**
   * @maxItems 512
   */
  loaded: InventoryInstance[];
}
export interface InventoryModel {
  model_key: string;
  vision: boolean;
}
export interface InventoryInstance {
  identifier: string;
  instance_reference: string;
  model_key: string;
  owned: boolean;
}
export interface BatchView {
  control: BatchControl;
  id: ProcessingBatchId;
  jobs: JobView[];
  revision: Revision;
}
export interface ExecutionReport {
  attempts: AttemptView[];
  calls: RuntimeCallView[];
  model_leases: ModelConsumerLeaseView[];
  reconciliation_required: boolean;
  retained_derivatives: DerivativeHoldView[];
}
export interface AttemptView {
  authorization_epoch: Revision;
  host_id: HostInstanceId;
  id: ProcessingAttemptId;
  job_id: ProcessingJobId;
  outcome_code?: string | null;
  state: AttemptState;
}
export interface RuntimeCallView {
  attempt_id: ProcessingAttemptId;
  generation: string;
  id: RuntimeCallId;
  kind: RuntimeCallKind;
  load_configuration?: LoadConfigurationStatus | null;
  load_reported_applied?: LoadConfigurationView | null;
  load_requested?: LoadConfigurationView | null;
  native_ttl_requested_seconds?: number | null;
  native_ttl_verification?: string | null;
  outcome_code?: string | null;
  state: RuntimeCallState;
}
export interface LoadConfigurationView {
  context_length?: number | null;
  eval_batch_size?: number | null;
  flash_attention?: boolean | null;
  k_cache_type?: string | null;
  max_parallel_predictions?: number | null;
  offload_kv_cache_to_gpu?: boolean | null;
  v_cache_type?: string | null;
}
export interface ModelConsumerLeaseView {
  attempt_id: ProcessingAttemptId;
  generation: string;
  id: ModelConsumerLeaseId;
  profile_fingerprint: string;
  state: ModelConsumerLeaseState;
}
export interface DerivativeHoldView {
  asset_id: AssetId;
  attempt_id: ProcessingAttemptId;
  operation_id: OperationId;
}
export interface AppError {
  code: ErrorCode;
  retryable: boolean;
  safe_message: SafeMessage;
}
