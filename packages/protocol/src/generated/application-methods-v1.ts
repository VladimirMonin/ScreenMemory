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
    };
export type ConsentId = string;
export type ProbeFixture = 'synthetic_menu_v1' | 'embedding_text_v1';
export type ProfileId = string;
export type Revision = string;
export type ConsentLifetime = number;

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
