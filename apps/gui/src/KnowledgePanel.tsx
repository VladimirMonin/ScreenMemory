import { invoke } from '@tauri-apps/api/core';
import { useEffect, useRef, useState } from 'react';
import type { CardView, CategoryCatalog, CategoryView, KnowledgeReply } from '../../../packages/protocol/src/generated/application-v1';
import type { KnowledgeRequest, CardPatch, CategoryChange } from '../../../packages/protocol/src/generated/application-methods-v1';

export type Mode = 'keep' | 'inherit' | 'set';
export type CardDraft = { title: string; description: string; categoryId: string; titleMode: Mode; descriptionMode: Mode; categoryMode: Mode };
export function editRequest(card: CardView, catalog: CategoryCatalog, draft: CardDraft, operationId: string): KnowledgeRequest {
  const patch: CardPatch = {
    title: draft.titleMode === 'set' ? { op: 'set', value: draft.title } : { op: draft.titleMode },
    description: draft.descriptionMode === 'set' ? { op: 'set', value: draft.description } : { op: draft.descriptionMode },
    category_id: draft.categoryMode === 'set' ? { op: 'set', value: draft.categoryId || null } : { op: draft.categoryMode },
  };
  return { action: 'edit', operation_id: operationId, record_id: card.record_id,
    expected_record_revision: card.record_revision, expected_catalog_revision: catalog.revision,
    expected_category_revision: draft.categoryMode === 'set' && draft.categoryId ? catalog.items.find(c => c.id === draft.categoryId)?.revision ?? null : null, patch };
}
export function pendingEdit(previous: KnowledgeRequest | null, card: CardView, catalog: CategoryCatalog, draft: CardDraft, operationId: string, explicitRetry: boolean): KnowledgeRequest {
  return !explicitRetry && previous ? previous : editRequest(card, catalog, draft, operationId);
}
const sourceLabels = { user: 'ручное', ai: 'AI', fallback: 'техническое', none: 'нет' };
export default function KnowledgePanel({ recordId }: { recordId: string | null }) {
  const [card, setCard] = useState<CardView | null>(null);
  const [catalog, setCatalog] = useState<CategoryCatalog | null>(null);
  const [current, setCurrent] = useState<CardView | null>(null);
  const [title, setTitle] = useState(''); const [description, setDescription] = useState(''); const [categoryId, setCategoryId] = useState('');
  const [titleMode, setTitleMode] = useState<Mode>('keep'); const [descriptionMode, setDescriptionMode] = useState<Mode>('keep'); const [categoryMode, setCategoryMode] = useState<Mode>('keep');
  const [notice, setNotice] = useState(''); const [busy, setBusy] = useState(false);
  const [name, setName] = useState(''); const [categoryDescription, setCategoryDescription] = useState('');
  const [editingCategory, setEditingCategory] = useState<CategoryView | null>(null);
  const pending = useRef<KnowledgeRequest | null>(null); const categoryPending = useRef<KnowledgeRequest | null>(null);
  const generation = useRef(0);
  function resetDraft(value: CardView) {
    setTitle(value.effective.title); setDescription(value.effective.description); setCategoryId(value.effective.category_id ?? '');
    setTitleMode('keep'); setDescriptionMode('keep'); setCategoryMode('keep'); pending.current = null; setCurrent(null);
  }
  async function call(request: KnowledgeRequest) { return invoke<KnowledgeReply>('gui_knowledge', { request }); }
  useEffect(() => {
    const token = ++generation.current; setCard(null); setCurrent(null); setCatalog(null); setBusy(false); pending.current = null; categoryPending.current = null; setNotice('');
    void (async () => {
      try {
        const cats = await call({ action: 'categories' });
        if (token !== generation.current) return;
        if (cats.kind === 'categories') setCatalog(cats.value);
        if (recordId) {
          const reply = await call({ action: 'read', record_id: recordId });
          if (token !== generation.current) return;
          if (reply.kind === 'card') { setCard(reply.value); resetDraft(reply.value); }
        }
      } catch { if (token === generation.current) setNotice('Карточка недоступна.'); }
    })();
    return () => { generation.current += 1; };
  }, [recordId]);
  function changed() { pending.current = null; }
  async function save(retryAgainstCurrent = false) {
    if (!card || !catalog) return;
    const token = generation.current;
    const base = retryAgainstCurrent && current ? current : card;
    pending.current = pendingEdit(pending.current, base, catalog, { title, description, categoryId, titleMode, descriptionMode, categoryMode }, crypto.randomUUID(), retryAgainstCurrent);
    setBusy(true); setNotice('');
    try {
      const reply = await call(pending.current);
      if (token !== generation.current) return;
      if (reply.kind === 'edited') { setCard(reply.value.card); resetDraft(reply.value.card); setNotice(reply.value.card.projection_state === 'current' ? 'Сохранено. FTS5 актуален; обновите поиск.' : 'Сохранено. Проекция недоступна.'); }
    } catch (error) {
      if (token !== generation.current) return;
      if (String(error) === 'REVISION_CONFLICT') {
        setNotice('Конфликт версии. Черновик сохранён в памяти; сравните актуальные поля.');
        try {
          const [reply, cats] = await Promise.all([call({ action: 'read', record_id: card.record_id }), call({ action: 'categories' })]);
          if (token !== generation.current) return;
          if (reply.kind === 'card') setCurrent(reply.value);
          if (cats.kind === 'categories') setCatalog(cats.value);
        } catch { if (token === generation.current) setNotice('Конфликт версии. Черновик сохранён; актуальная версия пока недоступна.'); }
      } else setNotice('Правка отклонена или ответ потерян. Повтор без изменений использует ту же операцию.');
    } finally { if (token === generation.current) setBusy(false); }
  }
  async function categoryMutation(change: CategoryChange) {
    if (!catalog) return;
    const token = generation.current;
    categoryPending.current = { action: 'category', operation_id: crypto.randomUUID(), expected_catalog_revision: catalog.revision, change };
    await retryCategory(token);
  }
  async function retryCategory(token = generation.current) {
    if (!categoryPending.current) return;
    setBusy(true); setNotice('');
    try {
      await call(categoryPending.current);
      if (token !== generation.current) return;
      categoryPending.current = null; setEditingCategory(null); setName(''); setCategoryDescription('');
      const cats = await call({ action: 'categories' });
      if (token !== generation.current) return;
      if (cats.kind === 'categories') setCatalog(cats.value);
      // Never replace a local draft after category maintenance. Show current snapshot alongside it.
      if (card) { const reply = await call({ action: 'read', record_id: card.record_id }); if (token === generation.current && reply.kind === 'card') setCurrent(reply.value); }
      setNotice('Каталог сохранён.');
    } catch (error) { if (token === generation.current) setNotice(String(error) === 'REVISION_CONFLICT' ? 'Конфликт каталога. Обновите каталог и повторите явно.' : 'Категория отклонена или ответ потерян.'); }
    finally { if (token === generation.current) setBusy(false); }
  }
  async function reloadCatalog() {
    try { const reply = await call({ action: 'categories' }); if (reply.kind === 'categories') { setCatalog(reply.value); categoryPending.current = null; } } catch { setNotice('Каталог недоступен.'); }
  }
  return <section className="knowledge-panel" aria-label="Карточка и категории">
    <h2>Карточка и категории</h2>
    <p>Ручные поля и FTS5 работают при AI Off. OCR/анализ ещё не подключены.</p>
    {card ? <>
      <h3>{card.effective.title}</h3>
      <p className="card-description">{card.effective.description || 'Описание не задано'}</p>
      <p>Категория: {card.effective.category_label ?? 'без категории'} · Источники: название — {sourceLabels[card.effective.title_source]}, описание — {sourceLabels[card.effective.description_source]}, категория — {sourceLabels[card.effective.category_source]}</p>
      <p>Версия записи {card.record_revision} · Источник поиска {card.content_revision} · Индекс: {card.projection_state === 'current' ? 'FTS5 актуален' : 'обработчик недоступен'}</p>
      <fieldset disabled={busy}>
        <legend>Независимые ручные поля</legend>
        <label>Название<input aria-label="Название карточки" value={title} onChange={e => { changed(); setTitleMode('set'); setTitle(e.target.value); }} /></label>
        <label>Режим названия<select value={titleMode} onChange={e => { changed(); setTitleMode(e.target.value as Mode); }}><option value="keep">Keep — оставить</option><option value="inherit">Inherit — наследовать</option><option value="set">Set — ручное</option></select></label>
        <label>Описание<textarea aria-label="Описание карточки" value={description} onChange={e => { changed(); setDescriptionMode('set'); setDescription(e.target.value); }} /></label>
        <label>Режим описания<select value={descriptionMode} onChange={e => { changed(); setDescriptionMode(e.target.value as Mode); }}><option value="keep">Keep — оставить</option><option value="inherit">Inherit — наследовать</option><option value="set">Set — включая пустое</option></select></label>
        <label>Категория<select aria-label="Категория карточки" value={categoryId} onChange={e => { changed(); setCategoryMode('set'); setCategoryId(e.target.value); }}><option value="">Без категории — Set(null)</option>{catalog?.items.filter(c => c.state === 'active' || c.id === card.effective.category_id).map(c => <option key={c.id} value={c.id} disabled={c.state !== 'active'}>{c.name}{c.state === 'archived' ? ' (архив)' : ''}</option>)}</select></label>
        <label>Режим категории<select value={categoryMode} onChange={e => { changed(); setCategoryMode(e.target.value as Mode); }}><option value="keep">Keep — оставить</option><option value="inherit">Inherit — наследовать</option><option value="set">Set — ручное / без категории</option></select></label>
        <button type="button" onClick={() => { void save(); }}>Сохранить карточку</button>
      </fieldset>
      {current && <aside aria-label="Актуальная версия"><h4>Актуальная версия {current.record_revision}</h4><p>{current.effective.title}</p><p className="card-description">{current.effective.description}</p><p>{current.effective.category_label ?? 'без категории'}</p><button disabled={busy} onClick={() => { setCard(current); resetDraft(current); }}>Перечитать и отбросить черновик</button><button disabled={busy} onClick={() => { void save(true); }}>Явно сохранить черновик поверх версии {current.record_revision}</button></aside>}
    </> : <p>Выберите сохранённое изображение для карточки.</p>}
    <details><summary>Мои категории ({catalog?.items.filter(c => c.state === 'active').length ?? '…'})</summary>
      <p>Архив сохраняет ссылки; удаление явно очищает назначенные карточки. Удалённый UUID не восстанавливается.</p>
      <button disabled={busy} onClick={() => { void reloadCatalog(); }}>Обновить каталог</button>
      <ul>{catalog?.items.map(c => <li key={c.id}><strong>{c.name}</strong> · {c.state} · {c.assigned_records} записей · v{c.revision}
        {c.state !== 'deleted' && <><button disabled={busy} onClick={() => { setEditingCategory(c); setName(c.name); setCategoryDescription(c.description); }}>Изменить</button><button disabled={busy} onClick={() => { void categoryMutation({ action: 'state', category_id: c.id, expected_revision: c.revision, state: c.state === 'active' ? 'archived' : 'active', expected_assigned_records: c.assigned_records }); }}>{c.state === 'active' ? 'В архив' : 'Восстановить'}</button><button disabled={busy} onClick={() => { void categoryMutation({ action: 'state', category_id: c.id, expected_revision: c.revision, state: 'deleted', expected_assigned_records: c.assigned_records }); }}>Удалить и очистить {c.assigned_records} назначений</button></>}
      </li>)}</ul>
      <fieldset disabled={busy}><legend>{editingCategory ? 'Изменить категорию' : 'Создать категорию'}</legend><label>Имя категории<input value={name} onChange={e => setName(e.target.value)} /></label><label>Назначение<textarea value={categoryDescription} onChange={e => setCategoryDescription(e.target.value)} /></label><button onClick={() => { void categoryMutation(editingCategory ? { action: 'rename', category_id: editingCategory.id, expected_revision: editingCategory.revision, name, description: categoryDescription } : { action: 'create', name, description: categoryDescription }); }}>Сохранить категорию</button>{editingCategory && <button onClick={() => { setEditingCategory(null); setName(''); setCategoryDescription(''); }}>Отмена изменения</button>}</fieldset>
      <button disabled={busy || !categoryPending.current} onClick={() => { void retryCategory(); }}>Повторить ту же операцию каталога</button>
    </details>
    {notice && <p role="status">{notice}</p>}
  </section>;
}
