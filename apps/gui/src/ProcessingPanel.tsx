import { invoke } from '@tauri-apps/api/core';
import { useRef, useState } from 'react';
import type { ProcessingReply, ProcessingOverview, InventoryView, BatchView, ExecutionReport } from '../../../packages/protocol/src/generated/application-v1';
import type { ProcessingRequest, ProcessingPolicy, ModelProfile } from '../../../packages/protocol/src/generated/application-methods-v1';
import { canonicalAssetId } from './model';

type ProbePlan = Extract<ProcessingRequest, { action: 'plan' }>;
/** Reopen/lost-response replay is exact; modified local data cannot mint a load authority. */
export function stableProbePlan(raw: string | null, candidate: ProbePlan): ProbePlan {
  try {
    const prior = JSON.parse(raw ?? 'null') as ProbePlan | null;
    if (prior && Object.keys(prior).sort().join(',') === 'action,expected_policy_revision,kind,operation_id,profile_id,records' && prior.action === 'plan' && prior.kind === 'technical_owner_self_test' && canonicalAssetId(prior.operation_id) && prior.profile_id === candidate.profile_id && prior.expected_policy_revision === candidate.expected_policy_revision && Array.isArray(prior.records) && prior.records.length === 1 && prior.records[0] === candidate.records[0]) return prior;
  } catch { /* Invalid local receipt is not dispatched. */ }
  return candidate;
}

/** Read UI is lazy. Only explicit buttons change policy, save profiles, inventory or plan work. */
export default function ProcessingPanel({ recordId, libraryId }: { recordId: string | null; libraryId: string | null }) {
  const [overview, setOverview] = useState<ProcessingOverview | null>(null);
  const [inventory, setInventory] = useState<InventoryView | null>(null);
  const [batch, setBatch] = useState<BatchView | null>(null);
  const [execution, setExecution] = useState<ExecutionReport | null>(null);
  const [notice, setNotice] = useState('Off по умолчанию. Настройки/статус не загружают модель.');
  const [busy, setBusy] = useState(false);
  const pending = useRef(false);
  const [policy, setPolicy] = useState<ProcessingPolicy>({ mode: 'off', background_loading: 'forbid', query_loading: 'ask', queue_paused: false, retention: { kind: 'no_host_eviction' } });
  const [profileId, setProfileId] = useState('');
  const [profile, setProfile] = useState<ModelProfile>({ id: crypto.randomUUID(), model_key: '', context_length: 4096, eval_batch_size: 512, flash_attention: true, offload_kv_cache_to_gpu: true, k_cache: 'f16', v_cache: 'f16', native_ttl_seconds: null, max_generation_tokens: 4000, temperature: 0, reasoning: 'backend_default' });
  const [profileRevision, setProfileRevision] = useState('0');
  const operation = useRef<{ key: string; request: ProcessingRequest } | null>(null);
  async function call(request: ProcessingRequest): Promise<ProcessingReply | null> {
    if (pending.current) return null;
    pending.current = true; setBusy(true); setNotice('');
    try {
      const reply = await invoke<ProcessingReply>('gui_processing', { request });
      if (reply.kind === 'overview') { setOverview(reply.value); setPolicy(reply.value.policy.value); }
      if (reply.kind === 'inventory') setInventory(reply.value);
      if (reply.kind === 'batch') {
        setBatch(reply.value);
        setOverview((old) => old ? { ...old, jobs: old.jobs.map((row) => row.batch_id === reply.value.id ? { ...row, batch_revision: reply.value.revision, job: reply.value.jobs.find((j) => j.id === row.job.id) ?? row.job } : row) } : old);
        setNotice(`Batch ${reply.value.id} · r${reply.value.revision} · ${reply.value.control}`);
      }
      if (reply.kind === 'job') {
        setOverview((old) => old ? { ...old, jobs: old.jobs.map((row) => row.job.id === reply.value.id ? { ...row, job: reply.value } : row) } : old);
        setNotice(`Job ${reply.value.id} · ${reply.value.state}${reply.value.reason ? `:${reply.value.reason}` : ''} · r${reply.value.revision}`);
      }
      if (reply.kind === 'execution') setExecution(reply.value);
      if (reply.kind === 'policy') { setPolicy(reply.value.value); setNotice(`Policy revision ${reply.value.revision} · epoch ${reply.value.authorization_epoch}`); }
      if (reply.kind === 'profile') { setOverview((old) => old ? { ...old, profiles: [reply.value, ...old.profiles.filter((p) => p.value.id !== reply.value.value.id)].slice(0, 32) } : old); setProfile(reply.value.value); setProfileId(reply.value.value.id); setProfileRevision(reply.value.revision); setNotice(`Profile revision ${reply.value.revision} · fingerprint ${reply.value.fingerprint}`); }
      return reply;
    } catch (error) {
      // Native adapter returns only a closed safe code. Do not display raw exception payloads.
      const code = typeof error === 'string' && /^[A-Z_]{3,64}$/.test(error) ? error : 'PROCESSING_UNCONFIRMED';
      setNotice(`${code} · проверьте статус; потеря ответа не отменяет работу и не разрешает новый load.`);
      return null;
    } finally { pending.current = false; setBusy(false); }
  }
  async function refresh(before: string | null = null) { await call({ action: 'overview', before }); }
  function stable(request: ProcessingRequest, purpose: string): ProcessingRequest {
    const key = JSON.stringify({ purpose, request });
    if (operation.current?.key === key) return operation.current.request;
    const bound = 'operation_id' in request ? { ...request, operation_id: crypto.randomUUID() } : request;
    operation.current = { key, request: bound };
    return bound;
  }
  async function savePolicy() {
    if (!overview) return;
    const request = stable({ action: 'update_policy', operation_id: '', expected_revision: overview.policy.revision, value: policy }, 'policy');
    const reply = await call(request);
    if (reply?.kind === 'policy') setOverview({ ...overview, policy: reply.value });
  }
  async function saveProfile() {
    if (!profile.model_key.trim()) { setNotice('Нужен точный local model key. Inventory — только отдельной кнопкой.'); return; }
    await call(stable({ action: 'save_profile', operation_id: '', expected_revision: profileRevision, value: profile }, 'profile'));
  }
  function chooseProfile(id: string) {
    setProfileId(id);
    const saved = overview?.profiles.find((p) => p.value.id === id);
    if (saved) { setProfile(saved.value); setProfileRevision(saved.revision); }
  }
  async function plan() {
    if (!recordId || !canonicalAssetId(recordId) || !canonicalAssetId(profileId) || !overview || !libraryId) return;
    const key = `sm-processing-plan-${libraryId}`;
    const candidate: ProbePlan = { action: 'plan', operation_id: crypto.randomUUID(), expected_policy_revision: overview.policy.revision, records: [recordId], kind: 'technical_owner_self_test', profile_id: profileId };
    const request = stableProbePlan(localStorage.getItem(key), candidate);
    // Stable operation saved before dispatch. A lost response can only repeat the exact request.
    localStorage.setItem(key, JSON.stringify(request));
    await call(request);
  }
  async function recoverPlan() {
    if (!libraryId) return;
    try { const raw = localStorage.getItem(`sm-processing-plan-${libraryId}`); if (!raw) return; const request = JSON.parse(raw) as ProbePlan; if (Object.keys(request).sort().join(',') !== 'action,expected_policy_revision,kind,operation_id,profile_id,records' || request.action !== 'plan' || request.kind !== 'technical_owner_self_test' || !canonicalAssetId(request.operation_id) || !request.profile_id || !canonicalAssetId(request.profile_id) || !Array.isArray(request.records) || request.records.length !== 1 || !canonicalAssetId(request.records[0])) return; await call(request); }
    catch { setNotice('Нет сохранённого корректного запроса.'); }
  }
  return <details open className="processing-panel" onToggle={(e) => { if (e.currentTarget.open && !overview && !pending.current) void refresh(); }}>
    <summary>Обработка · настройки, модели и очередь (S04)</summary>
    <p>Изображения/импорт/preview/экспорт независимы от AI. OCR/описания S05 пока не подключены: Analyze = blocked:handler_unavailable, не карточка.</p>
    <button disabled={busy} onClick={() => void refresh()}>Обновить настройки и очередь</button>
    <p role="status">{notice}</p>
    {overview && <>
      <p>Runtime: {overview.runtime_configured ? 'настроен host' : 'не настроен host'} · bridge {overview.bridge_state} · model.autostart=false · policy revision {overview.policy.revision} · authorization epoch {overview.policy.authorization_epoch}</p>
      {!overview.runtime_configured && <p>Настройте у остановленного host config/processing-runtime.json: library identity, явный numeric loopback port, credential либо явный guest. SDK берётся только из trusted sibling sdk-runtime; токен вводится native через stdin, не в этом WebView.</p>}
      <h3>Политика обработки</h3>
      <label>Режим<select value={policy.mode} disabled={busy} onChange={(e) => setPolicy({ ...policy, mode: e.target.value as ProcessingPolicy['mode'] })}><option value="off">AI выключен</option><option value="manual">Вручную</option><option value="automatic">Автоматически (S05 handler ещё отсутствует)</option></select></label>
      <label>Background load<select value={policy.background_loading} onChange={(e) => setPolicy({ ...policy, background_loading: e.target.value as ProcessingPolicy['background_loading'] })}><option value="forbid">Запрещён</option><option value="allow">Разрешён</option></select></label>
      <label>Query / explicit foreground load<select value={policy.query_loading} onChange={(e) => setPolicy({ ...policy, query_loading: e.target.value as ProcessingPolicy['query_loading'] })}><option value="ask">Ask (только host approval)</option><option value="forbid">Запрещён</option><option value="allow">Разрешён</option></select></label>
      <label><input type="checkbox" checked={policy.queue_paused} onChange={(e) => setPolicy({ ...policy, queue_paused: e.target.checked })} />Пауза admission, не cancel текущей работы</label>
      <button disabled={busy} onClick={() => void savePolicy()}>Сохранить политику</button>
      <p>Physical unload: Unsupported для всех причин. Host Idle / AfterBatch / KeepLoaded — Unsupported; только NoHostEviction (не гарантия retention). Lease release / stop Node ≠ eviction.</p>
      <details><summary>Профиль модели и inventory (сохранение не загружает)</summary>
      <label>Сохранённый профиль<select value={profileId} onChange={(e) => chooseProfile(e.target.value)}><option value="">Выберите явно</option>{overview.profiles.map((p) => <option key={p.value.id} value={p.value.id}>{p.value.model_key} · r{p.revision}</option>)}</select></label>
      <button disabled={busy} onClick={() => { setProfileId(''); setProfile({ ...profile, id: crypto.randomUUID(), model_key: '' }); setProfileRevision('0'); }}>Новый профиль</button>
      <label>Точный local model key<input value={profile.model_key} onChange={(e) => setProfile({ ...profile, model_key: e.target.value })} maxLength={512} /></label>
      <p>Profile ID {profile.id} · revision {profileRevision}</p>
      <label>Контекст<input type="number" min={1024} max={262144} value={profile.context_length} onChange={(e) => setProfile({ ...profile, context_length: Number(e.target.value) })} /></label>
      <label>Eval batch<input type="number" min={1} max={2048} value={profile.eval_batch_size} onChange={(e) => setProfile({ ...profile, eval_batch_size: Number(e.target.value) })} /></label>
      <label><input type="checkbox" checked={profile.flash_attention} onChange={(e) => setProfile({ ...profile, flash_attention: e.target.checked })} />Flash Attention</label>
      <label><input type="checkbox" checked={profile.offload_kv_cache_to_gpu} onChange={(e) => setProfile({ ...profile, offload_kv_cache_to_gpu: e.target.checked })} />KV на GPU</label>
      {(['k_cache', 'v_cache'] as const).map((field) => <label key={field}>{field}<select value={profile[field]} onChange={(e) => setProfile({ ...profile, [field]: e.target.value })}>{['f16', 'f32', 'q8', 'q4', 'q5'].map((v) => <option key={v}>{v}</option>)}</select></label>)}
      <label>Общий лимит генерации ≤4000<input type="number" min={1} max={4000} value={profile.max_generation_tokens} onChange={(e) => setProfile({ ...profile, max_generation_tokens: Number(e.target.value) })} /></label>
      <label>Temperature 0…1<input type="number" min={0} max={1} step={0.1} value={profile.temperature} onChange={(e) => setProfile({ ...profile, temperature: Number(e.target.value) })} /></label>
      <label>Reasoning<select value={profile.reasoning} onChange={(e) => setProfile({ ...profile, reasoning: e.target.value as ModelProfile['reasoning'] })}><option value="backend_default">BackendDefault (явно)</option><option disabled>Off — Unsupported</option><option disabled>Budget100 — Unsupported</option></select></label>
      <label>Native TTL seconds (пусто = не запрошен)<input type="number" min={1} max={86400} value={profile.native_ttl_seconds ?? ''} onChange={(e) => setProfile({ ...profile, native_ttl_seconds: e.target.value === '' ? null : Number(e.target.value) })} /></label>
      <p>Native TTL: requested / not_verified. Это server policy, не HostIdle/AfterBatch/KeepLoaded.</p>
      <button disabled={busy || !profile.model_key} onClick={() => void saveProfile()}>Сохранить профиль</button>
      <button disabled={busy || !overview.runtime_configured} onClick={() => void call({ action: 'inventory' })}>Явно прочитать inventory (без model load)</button>
      {inventory && <div><h4>Последний явный inventory snapshot — не ownership/adoption</h4><ul>{inventory.downloaded.map((m) => <li key={m.model_key}>{m.model_key} · vision {String(m.vision)} <button disabled={busy} onClick={() => setProfile({ ...profile, model_key: m.model_key })}>Выбрать ключ</button></li>)}</ul><p>Loaded в snapshot: {inventory.loaded.length}; чужие instances не изменяются.</p><ul>{inventory.loaded.map((m) => <li key={m.instance_reference}>{m.model_key} · {m.identifier} · reference {m.instance_reference} · {m.owned ? 'owned current generation' : 'foreign / not adopted'}</li>)}</ul></div>}
      </details>
      <h3>TechnicalOwnerSelfTest — явный запрос</h3>
      <p>Выбранная Saved запись: {recordId ?? 'не выбрана'}. Только технические metadata/usage; не описание и не knowledge. Может выполнить разрешённый load/inference.</p>
      <button disabled={busy || !recordId || !profileId || policy.mode === 'off'} onClick={() => void plan()}>Запустить technical self-test выбранного</button>
      <button disabled={busy} onClick={() => void recoverPlan()}>Проверить/повторить exact сохранённый запрос</button>
      <h3>Очередь · ≤32 jobs на страницу</h3>
      {overview.jobs.length === 0 && <p>Работ пока нет.</p>}
      <ul>{overview.jobs.map(({ job, batch_id, batch_revision }) => <li key={job.id}>
        <p>{job.kind} · {job.state}{job.reason ? `:${job.reason}` : ''} · attempt {job.attempt_count} · job {job.id} · r{job.revision}</p>
        <button disabled={busy} onClick={() => void call({ action: 'control_job', id: job.id, expected_revision: job.revision, control: 'cancel' })}>Cancel job</button>
        <button disabled={busy} onClick={() => void call({ action: 'control_job', id: job.id, expected_revision: job.revision, control: 'retry' })}>Explicit retry</button>
        <button disabled={busy} onClick={() => void call({ action: 'execution', id: job.id })}>Execution / requested-applied</button>
        <button disabled={busy} onClick={() => void call({ action: 'batch', id: batch_id })}>Batch status</button>
        {(['pause', 'resume', 'cancel'] as const).map((control) => <button disabled={busy} key={control} onClick={() => void call({ action: 'control_batch', id: batch_id, expected_revision: batch_revision, control })}>{control} batch</button>)}
      </li>)}</ul>
      {overview.next_before && <button disabled={busy} onClick={() => void refresh(overview.next_before)}>Следующие jobs</button>}
      {batch && <p>Batch {batch.id} · r{batch.revision} · {batch.control} · {batch.jobs.map((j) => `${j.id}: ${j.state}${j.reason ? `:${j.reason}` : ''}`).join(' · ')}</p>}
      <button disabled={busy} onClick={() => void call({ action: 'reconcile' })}>Readonly reconcile (не retry/settlement)</button>
      {execution && <pre className="processing-ledger">{JSON.stringify(execution, null, 2)}</pre>}
    </>}
  </details>;
}
