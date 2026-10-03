/* Generated from sm-protocol JSON Schema; do not edit. Runtime validation uses Ajv. */

export type KnownParams =
  | {
      method: 'system.status';
      params: EmptyParams;
    }
  | {
      method: 'system.doctor';
      params: DoctorRequest;
    }
  | {
      method: 'runtime.probe';
      params: RuntimeProbeRequest;
    }
  | {
      method: 'system.set_ai_policy';
      params: AiPolicyPatch;
    }
  | {
      method: 'system.authorize_probe';
      params: AuthorizeProbeRequest;
    }
  | {
      method: 'system.request_shutdown';
      params: EmptyParams;
    }
  | {
      method: 'capture.command';
      params: CaptureCommandParams;
    }
  | {
      method: 'library.asset.read';
      params: LibraryAssetRequest;
    }
  | {
      method: 'library.asset.preview';
      params: LibraryAssetRequest;
    }
  | {
      method: 'library.record.metadata';
      params: RecordMetadataRequest;
    }
  | {
      method: 'library.list_records';
      params: LibraryRecordsRequest;
    }
  | {
      method: 'library.import';
      params: ImportRequest;
    }
  | {
      method: 'library.import.status';
      params: ImportBatchRequest;
    }
  | {
      method: 'library.import.report';
      params: ImportReportPageRequest;
    }
  | {
      method: 'library.import.control';
      params: ImportControlRequest;
    }
  | {
      method: 'library.export';
      params: ExportRequest;
    };
export type ConsentId = string;
export type ProbeFixture = 'synthetic_menu_v1' | 'embedding_text_v1';
export type ProfileId = string;
export type Revision = string;
export type ConsentLifetime = number;
/**
 * S02 §3.1: closed vocabulary of one capture target.
 */
export type CaptureMode = 'region' | 'window' | 'monitor' | 'desktop';
export type OperationId = string;
export type AssetId = string;
export type RecordId = string;
export type ImportBatchId = string;
export type DuplicatePolicy = 'reuse_record' | 'keep_both';
export type CanonicalProfile = 'webp_exact' | 'avif_sdr90';
export type ImportControl = 'pause' | 'cancel_processing' | 'resume' | 'retry';
export type ExportFormat = 'png' | 'jpeg' | 'webp' | 'avif';
export type ExportMetadataPolicy = 'strip_private' | 'include_source_sidecar';

export interface EmptyParams {}
export interface DoctorRequest {
  include_runtime_inventory: boolean;
}
export interface RuntimeProbeRequest {
  allow_model_load: boolean;
  consent_id: ConsentId;
  fixture: ProbeFixture;
  profile_id: ProfileId;
}
export interface AiPolicyPatch {
  enabled: boolean;
  expected_revision: Revision;
}
export interface AuthorizeProbeRequest {
  allow_load: boolean;
  fixture: ProbeFixture;
  lifetime_seconds: ConsentLifetime;
  profile_id: ProfileId;
}
/**
 * S02 §5.2: parameters of `capture.command`.
 *
 * The initiator is deliberately absent: a client cannot declare itself a native hotkey.
 * The trusted host adapter records the trigger out of band.
 */
export interface CaptureCommandParams {
  mode: CaptureMode;
  operation_id: OperationId;
}
/**
 * S02 §10.1: bounded read of one committed canonical asset.
 */
export interface LibraryAssetRequest {
  asset_id: AssetId;
  offset: number;
}
export interface RecordMetadataRequest {
  record_id: RecordId;
}
/**
 * S02 §10.3: one bounded page of committed library records.
 */
export interface LibraryRecordsRequest {
  cursor?: string | null;
  limit: number;
}
export interface ImportRequest {
  batch_id: ImportBatchId;
  duplicates: DuplicatePolicy;
  profile: CanonicalProfile;
  source_paths: string[];
}
export interface ImportBatchRequest {
  batch_id: ImportBatchId;
}
export interface ImportReportPageRequest {
  after_ordinal: number;
  batch_id: ImportBatchId;
}
export interface ImportControlRequest {
  batch_id: ImportBatchId;
  control: ImportControl;
}
export interface ExportRequest {
  /**
   * None resolves Current exactly once. Replay never resolves again.
   */
  asset_id?: AssetId | null;
  /**
   * None selects the host-managed export registry (TTL 24h), never a wire cache path.
   */
  destination_path?: string | null;
  format: ExportFormat;
  /**
   * @minItems 3
   * @maxItems 3
   */
  jpeg_background?: [number, number, number] | null;
  max_side?: number | null;
  metadata_policy: ExportMetadataPolicy;
  operation_id: OperationId;
  record_id: RecordId;
}
