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
      method: 'library.list_records';
      params: LibraryRecordsRequest;
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
/**
 * S02 §10.3: one bounded page of committed library records.
 */
export interface LibraryRecordsRequest {
  cursor?: string | null;
  limit: number;
}
