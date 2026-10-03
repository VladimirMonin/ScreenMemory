import * as Ajv2020Module from 'ajv/dist/2020.js';
import * as formatsModule from 'ajv-formats';
import type { FormatsPlugin } from 'ajv-formats';
import application from '../../../schemas/application-v1.schema.json' with { type: 'json' };
import methods from '../../../schemas/application-methods-v1.schema.json' with { type: 'json' };
import bridge from '../../../schemas/bridge-v1.schema.json' with { type: 'json' };
import errors from '../../../schemas/foundation-errors.schema.json' with { type: 'json' };
import type { ApplicationFrame, RequestEnvelope, Response } from './generated/application-v1.js';
import type { KnownParams } from './generated/application-methods-v1.js';
import type { BridgeFrame } from './generated/bridge-v1.js';

export type { ApplicationFrame, RequestEnvelope, Response, KnownParams, BridgeFrame };
export type { ProcessingOverview, QueuedJob, InventoryView, InventoryModel, InventoryInstance, ProcessingReply, PolicyView, ProfileView, BatchView, JobView,
  TechnicalProbeResult, ExecutionReport, AttemptView, AttemptState,
  RuntimeCallView, RuntimeCallKind, RuntimeCallState, ModelConsumerLeaseView,
  ModelConsumerLeaseState, DerivativeHoldView, ModelConsumerLeaseId,
  ProcessingAttemptId, RuntimeCallId, ProcessingBatchId, ProcessingJobId,
  ModelProfileId } from './generated/application-v1.js';
export type { ProcessingRequest, ProcessingPolicy, ModelProfile } from './generated/application-methods-v1.js';
export type { ImportReport, ImportReceipt, ExportReceipt, ExportExistence, ExportAppliedParameters, RecordMetadata } from './generated/application-v1.js';
export type { ImportRequest, ImportBatchRequest, ImportReportPageRequest, ImportControlRequest, ExportRequest, RecordMetadataRequest } from './generated/application-methods-v1.js';
export type { CardView, CardReceipt, EffectiveCard, KnowledgeSource, CategoryCatalog, CategoryView, CategoryReceipt, KnowledgeReply, FieldSource, CategoryState } from './generated/application-v1.js';
export type { KnowledgeRequest, CardPatch, CardEdit, CategoryChange, CategoryMutation } from './generated/application-methods-v1.js';
export type Validation<T> = { readonly ok: true; readonly value: T } | { readonly ok: false; readonly reason: 'invalid_application' | 'invalid_method' | 'invalid_bridge' | 'invalid_error_code' };

// No coercion/defaults/removal, no remote refs or schema fetch; errors are never exposed.
const Ajv2020 = Ajv2020Module.default as unknown as typeof Ajv2020Module.Ajv2020;
const addFormats = formatsModule.default as unknown as FormatsPlugin;
const ajv = new Ajv2020({ strict: true, validateFormats: true,
  allErrors: false, coerceTypes: false, removeAdditional: false, useDefaults: false });
addFormats(ajv, { mode: 'full' });
// Rust unsigned integer formats are not part of the ajv-formats registry.
ajv.addFormat('uint8', { type: 'number', validate: (n: number) => Number.isInteger(n) && n >= 0 && n <= 255 });
ajv.addFormat('uint16', { type: 'number', validate: (n: number) => Number.isInteger(n) && n >= 0 && n <= 65535 });
ajv.addFormat('uint32', { type: 'number', validate: (n: number) => Number.isInteger(n) && n >= 0 && n <= 4294967295 });
// SDK usage counters use Rust u64; upper-exclusive avoids rounding UINT64_MAX to 2^64.
ajv.addFormat('uint64', { type: 'number', validate: (n: number) => Number.isInteger(n) && n >= 0 && n < 2 ** 64 });
const appValidate = ajv.compile<ApplicationFrame>(application);
const methodValidate = ajv.compile<KnownParams>(methods);
const bridgeValidate = ajv.compile<BridgeFrame>(bridge);
const errorValidate = ajv.compile<string>(errors);

export function validateApplication(input: unknown): Validation<ApplicationFrame> {
  return appValidate(input) ? { ok: true, value: input } : { ok: false, reason: 'invalid_application' };
}

/** The envelope alone never authorizes a method. Caller must separately enforce host policy. */
export function validateApplicationRequest(input: unknown): Validation<RequestEnvelope> {
  if (!appValidate(input) || !input || typeof input !== 'object' || !('method' in input)) {
    return { ok: false, reason: 'invalid_application' };
  }
  const request = input as RequestEnvelope;
  if (!methodValidate({ method: request.method, params: request.params })) {
    return { ok: false, reason: 'invalid_method' };
  }
  return { ok: true, value: request };
}

export function validateApplicationResponse(input: unknown): Validation<Response> {
  if (!appValidate(input) || !input || typeof input !== 'object' || !('kind' in input)) {
    return { ok: false, reason: 'invalid_application' };
  }
  return { ok: true, value: input as Response };
}

export function validateBridge(input: unknown): Validation<BridgeFrame> {
  return bridgeValidate(input) ? { ok: true, value: input } : { ok: false, reason: 'invalid_bridge' };
}

export function validateErrorCode(input: unknown): Validation<string> {
  return errorValidate(input) ? { ok: true, value: input } : { ok: false, reason: 'invalid_error_code' };
}
