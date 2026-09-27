/* Generated from sm-protocol JSON Schema; do not edit. Runtime validation uses Ajv. */

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
