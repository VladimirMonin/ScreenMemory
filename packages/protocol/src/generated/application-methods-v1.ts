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
    }
  | {
      method: 'processing.request';
      params: ProcessingRequest;
    }
  | {
      method: 'knowledge.request';
      params: KnowledgeRequest;
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
export type ProcessingRequest =
  | {
      action: 'policy';
    }
  | {
      action: 'overview';
      before?: ProcessingJobId | null;
    }
  | {
      action: 'inventory';
    }
  | {
      action: 'update_policy';
      expected_revision: Revision;
      operation_id: OperationId;
      value: ProcessingPolicy;
    }
  | {
      action: 'save_profile';
      expected_revision: Revision;
      operation_id: OperationId;
      value: ModelProfile;
    }
  | {
      action: 'profile';
      id: ModelProfileId;
    }
  | {
      action: 'plan';
      expected_policy_revision: Revision;
      kind: WorkKind;
      operation_id: OperationId;
      profile_id?: ModelProfileId | null;
      /**
       * @minItems 1
       * @maxItems 512
       */
      records: [RecordId, ...RecordId[]];
    }
  | {
      action: 'batch';
      id: ProcessingBatchId;
    }
  | {
      action: 'control_batch';
      control: BatchControl;
      expected_revision: Revision;
      id: ProcessingBatchId;
    }
  | {
      action: 'control_job';
      control: JobControl;
      expected_revision: Revision;
      id: ProcessingJobId;
    }
  | {
      action: 'execution';
      id: ProcessingJobId;
    }
  | {
      action: 'reconcile';
    };
export type ProcessingJobId = string;
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
export type ModelProfileId = string;
export type CacheType = 'f16' | 'f32' | 'q8' | 'q4' | 'q5';
export type ReasoningMode = 'backend_default' | 'off' | 'budget100';
export type WorkKind = 'analyze' | 'technical_owner_self_test' | 'technical_owner_background_self_test';
export type ProcessingBatchId = string;
export type BatchControl = 'pause' | 'resume' | 'cancel';
export type JobControl = 'cancel' | 'retry';
export type KnowledgeRequest =
  | {
      action: 'read';
      record_id: KnowledgeId;
    }
  | {
      action: 'edit';
      expected_catalog_revision: SqlRevision;
      expected_category_revision?: SqlRevision | null;
      expected_record_revision: SqlRevision;
      operation_id: KnowledgeId;
      patch: CardPatch;
      record_id: KnowledgeId;
    }
  | {
      action: 'categories';
    }
  | {
      action: 'category';
      change: CategoryChange;
      expected_catalog_revision: SqlRevision;
      operation_id: KnowledgeId;
    };
export type KnowledgeId = string;
export type SqlRevision = string;
export type CardDescription = string;
export type CardTitle = string;
export type CategoryChange =
  | {
      action: 'create';
      description: CategoryDescription;
      name: CategoryName;
    }
  | {
      action: 'rename';
      category_id: KnowledgeId;
      description: CategoryDescription;
      expected_revision: SqlRevision;
      name: CategoryName;
    }
  | {
      action: 'state';
      category_id: KnowledgeId;
      expected_assigned_records: number;
      expected_revision: SqlRevision;
      state: CategoryState;
    };
export type CategoryDescription = string;
export type CategoryName = string;
export type CategoryState = 'active' | 'archived' | 'deleted';

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
export interface ProcessingPolicy {
  background_loading: LoadPermission;
  mode: ProcessingMode;
  query_loading: QueryLoadPermission;
  queue_paused: boolean;
  retention: HostRetention;
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
export interface CardPatch {
  category_id?:
    | {
        op: 'keep';
      }
    | {
        op: 'inherit';
      }
    | {
        op: 'set';
        value?: KnowledgeId | null;
      };
  description?:
    | {
        op: 'keep';
      }
    | {
        op: 'inherit';
      }
    | {
        op: 'set';
        value: CardDescription;
      };
  title?:
    | {
        op: 'keep';
      }
    | {
        op: 'inherit';
      }
    | {
        op: 'set';
        value: CardTitle;
      };
}
