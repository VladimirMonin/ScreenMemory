import { LMStudioClient, type LLM, type LLMLoadModelConfig, type LLMPredictionConfig, type LLMLlamaCacheQuantizationType } from '@lmstudio/sdk';
import { Validator } from 'jsonschema';

export class SafeFailure extends Error {
  public code: string;
  public settlement: 'settled' | 'uncertain';
  constructor(code: string, settlement: 'settled' | 'uncertain' = 'settled') {
    super(code); this.code = code; this.settlement = settlement;
  }
}
export interface LoadPlan {
  intent_id: string; identifier: string; model_key: string;
  context_length: number; eval_batch_size: number;
  flash_attention: boolean; offload_kv_cache_to_gpu: boolean;
  k_cache_type: 'F16' | 'F32' | 'Q8_0' | 'Q4_0' | 'Q4_1' | 'Q5_0' | 'Q5_1' | 'IQ4_NL';
  v_cache_type: LoadPlan['k_cache_type']; ttl_seconds: number | null;
}
export interface VisionPlan {
  instance_reference: string; prompt: string; schema: Record<string, unknown>;
  max_tokens: number; temperature: number;
  reasoning: 'backend_default' | 'off' | 'budget100';
}
// No runtime/model pair has been behavior-verified yet. Source presence is NOT a capability.
const verifiedReasoning = false;
const cacheMapping: Record<LoadPlan['k_cache_type'], LLMLlamaCacheQuantizationType> = {
  F16: 'f16', F32: 'f32', Q8_0: 'q8_0', Q4_0: 'q4_0', Q4_1: 'q4_1',
  Q5_0: 'q5_0', Q5_1: 'q5_1', IQ4_NL: 'iq4_nl' };
const quiet = { debug() {}, info() {}, warn() {}, error() {} };
export class SdkAdapter {
  private readonly client: LMStudioClient;
  private readonly owned = new Map<string, LLM>();
  private readonly configured = new Set<string>();
  constructor(endpoint: string, token: string) {
    this.client = new LMStudioClient({ baseUrl: endpoint,
      ...(token ? { apiToken: token } : { clientIdentifier: 'guest:screenmemory', clientPasskey: 'explicit-no-token' }),
      logger: quiet, verboseErrorMessages: false });
  }
  async inventory() {
    const downloaded = await this.client.system.listDownloadedModels('llm');
    const loaded = await this.client.llm.listLoaded();
    const infos = await Promise.all(loaded.map(m => m.getModelInfo()));
    return { downloaded: downloaded.filter(m => m.deviceIdentifier === null).map(m => ({ model_key: m.modelKey, vision: m.vision })),
      loaded: infos.filter(m => m.deviceIdentifier === null).map(m => ({
        model_key: m.modelKey, identifier: m.identifier, instance_reference: m.instanceReference,
        owned: this.owned.has(m.instanceReference) })) };
  }
  async load(plan: LoadPlan, signal: AbortSignal) {
    // A collision never adopts a loaded instance. The caller has persisted this exact intent.
    const inventory = await this.client.llm.listLoaded();
    if (inventory.some(m => m.identifier === plan.identifier)) throw new SafeFailure('IDENTIFIER_COLLISION');
    if (this.owned.size >= 1) throw new SafeFailure('OWNED_INSTANCE_LIMIT');
    const downloaded = await this.client.system.listDownloadedModels('llm');
    const candidates = downloaded.filter(m => m.modelKey === plan.model_key && m.deviceIdentifier === null && m.vision);
    if (candidates.length !== 1 || plan.context_length > candidates[0].maxContextLength) throw new SafeFailure('EXACT_LOCAL_MODEL_REQUIRED');
    const config: LLMLoadModelConfig = { contextLength: plan.context_length,
      evalBatchSize: plan.eval_batch_size, flashAttention: plan.flash_attention,
      offloadKVCacheToGpu: plan.offload_kv_cache_to_gpu,
      llamaKCacheQuantizationType: cacheMapping[plan.k_cache_type], llamaVCacheQuantizationType: cacheMapping[plan.v_cache_type],
      maxParallelPredictions: 1 };
    let model: LLM;
    try {
      model = await this.client.llm.load(plan.model_key, { identifier: plan.identifier, config,
        signal, verbose: false, deviceIdentifier: null,
        ...(plan.ttl_seconds === null ? {} : { ttl: plan.ttl_seconds }) });
    } catch { throw new SafeFailure('LOAD_UNCERTAIN', 'uncertain'); }
    if (model.identifier !== plan.identifier || model.modelKey !== plan.model_key || !model.vision
        ) throw new SafeFailure('INSTANCE_MISMATCH', 'uncertain');
    this.owned.set(model.instanceReference, model);
    await this.specific(model.instanceReference);
    const applied = await model.getLoadConfig();
    const matches = Object.entries(config).every(([key, value]) => applied[key as keyof LLMLoadModelConfig] === value);
    if (matches) this.configured.add(model.instanceReference);
    return { instance_reference: model.instanceReference, identifier: model.identifier,
      model_key: model.modelKey, intent_id: plan.intent_id, requested: config, applied,
      configuration: matches ? 'reported_applied' : 'mismatch',
      native_ttl: { requested_seconds: plan.ttl_seconds, verification: 'not_verified' } };
  }
  private async specific(reference: string) {
    const model = this.owned.get(reference);
    if (!model) throw new SafeFailure('NOT_OWNED');
    const info = await model.getModelInfo();
    if (info.instanceReference !== reference || info.modelKey !== model.modelKey || info.identifier !== model.identifier
        || info.deviceIdentifier !== null) throw new SafeFailure('INSTANCE_CHANGED', 'uncertain');
    return model;
  }
  async observe(reference: string) {
    const model = await this.specific(reference);
    return { instance_reference: reference, applied: await model.getLoadConfig() };
  }
  async unload(_reference: string): Promise<never> {
    // Published unload is identifier-only, not atomic compare-and-unload. Never race a foreign instance.
    throw new SafeFailure('UNLOAD_IDENTITY_ATOMICITY_UNSUPPORTED');
  }
  async vision(plan: VisionPlan, pngBase64: string, signal: AbortSignal) {
    if (plan.reasoning !== 'backend_default' && !verifiedReasoning) throw new SafeFailure('REASONING_UNSUPPORTED');
    const model = await this.specific(plan.instance_reference);
    if (!this.configured.has(plan.instance_reference)) throw new SafeFailure('CONFIGURATION_MISMATCH');
    const image = await this.client.files.prepareImageBase64('inference.png', pngBase64);
    if (signal.aborted) throw new SafeFailure('CANCELLED');
    const config: LLMPredictionConfig = { maxTokens: plan.max_tokens, temperature: plan.temperature,
      contextOverflowPolicy: 'stopAtLimit', structured: { type: 'json', jsonSchema: plan.schema },
      ...(plan.reasoning === 'budget100' ? { reasoningBudget: 100 } : {}) };
    const ongoing = model.respond({ role: 'user', content: plan.prompt, images: [image] }, config);
    const cancel = () => { void ongoing.cancel().catch(() => {}); };
    signal.addEventListener('abort', cancel, { once: true });
    if (signal.aborted) cancel();
    try {
      let bytes = 0;
      for await (const fragment of ongoing) {
        bytes += new TextEncoder().encode(fragment.content).byteLength;
        if (bytes > 128 * 1024) { cancel(); throw new SafeFailure('OUTPUT_LIMIT', 'uncertain'); }
      }
      const result = await ongoing.result();
      if (result.modelInfo.instanceReference !== plan.instance_reference) throw new SafeFailure('INSTANCE_CHANGED', 'uncertain');
      const complete = !signal.aborted && (result.stats.stopReason === 'eosFound' || result.stats.stopReason === 'stopStringFound');
      let value: unknown = null;
      if (complete) {
        try { value = JSON.parse(result.content); } catch { throw new SafeFailure('STRUCTURED_INVALID'); }
        if (!new Validator().validate(value, plan.schema).valid) throw new SafeFailure('STRUCTURED_INVALID');
      }
      if ((result.stats.predictedTokensCount ?? 0) > plan.max_tokens) throw new SafeFailure('TOKEN_LIMIT_VIOLATION');
      return { instance_reference: plan.instance_reference, settlement: 'settled', complete,
        value: complete ? value : null, stop_reason: result.stats.stopReason,
        usage: { prompt_tokens: result.stats.promptTokensCount ?? null,
          generated_tokens: result.stats.predictedTokensCount ?? null, reasoning_tokens: null } };
    } finally { signal.removeEventListener('abort', cancel); }
  }
  async close() { await this.client[Symbol.asyncDispose](); }
}
