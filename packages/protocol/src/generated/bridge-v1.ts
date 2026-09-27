/* Generated from sm-protocol JSON Schema; do not edit. Runtime validation uses Ajv. */

export type BridgeFrame = BridgeProbeRequest | BridgeResponse;
export type ProbeFixture = 'synthetic_menu_v1' | 'embedding_text_v1';
export type ProfileId = string;
export type RequestId = string;
export type BridgeResponse =
  | {
      checks: CheckResult[];
      kind: 'result';
      request_id: RequestId;
    }
  | {
      error: BridgeError;
      kind: 'error';
      request_id: RequestId;
    };
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
export type BridgeErrorCode =
  'UNAVAILABLE' | 'UNSUPPORTED' | 'MODEL_NOT_INSTALLED' | 'MODEL_CONFIG_UNSUPPORTED' | 'WAIT_TIMEOUT';

export interface BridgeProbeRequest {
  allow_model_load: boolean;
  fixture: ProbeFixture;
  profile_id: ProfileId;
  protocol: ProtocolVersion;
  request_id: RequestId;
}
export interface ProtocolVersion {
  major: number;
  minor: number;
}
export interface CheckResult {
  check_id: CheckId;
  duration_ms?: number;
  evidence_id?: EvidenceId;
  outcome: CheckOutcome;
  safe_code?: ErrorCode;
}
export interface BridgeError {
  code: BridgeErrorCode;
  retryable: boolean;
}
