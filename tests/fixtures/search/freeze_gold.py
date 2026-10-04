"""Build the declared production AnyTerms/exact gold BEFORE search implementation.
No execution/results are inputs. IDs and relevance are declared synthetic card facts.
Run once into an ABSENT output file; checked-in gold-v1.json is the frozen artifact.
Legacy 11/18/20 quoted-AND experiment remains unchanged and is NOT this gate.
"""
import json
import pathlib
import sys

TOPICS = [
    ('миграция', 'migration', 'SQLite', '0xc0000005'),
    ('квитанция', 'receipt', 'Invoice', 'ZXQ472'),
    ('контекст', 'context', 'Runtime', 'Q8_0'),
    ('настройки', 'settings', 'Editor', 'settings.json'),
    ('версия', 'version', 'Release', 'v1.2.3'),
    ('ошибка', 'failure', 'Terminal', 'ERROR_ACCESS_DENIED'),
    ('маршрут', 'route', 'Framework', 'django.db'),
    ('компилятор', 'compiler', 'Build', 'C++'),
    ('язык', 'language', 'Project', 'C#'),
    ('память', 'memory', 'Capacity', '128K'),
    ('индекс', 'index', 'Search', 'FTS5'),
    ('цвет', 'color', 'Image', 'sRGB'),
    ('экспорт', 'export', 'Codec', 'AVIF90'),
    ('сервер', 'server', 'Network', 'localhost:1234'),
    ('очередь', 'queue', 'Jobs', 'job-184'),
    ('категория', 'category', 'Library', 'catalog_42'),
    ('профиль', 'profile', 'Model', 'profile-7'),
    ('снимок', 'snapshot', 'Capture', 'frame_81'),
    ('восстановление', 'recovery', 'Backup', 'backup.tar'),
    ('разрешение', 'permission', 'Security', 'scope:read'),
]

def rid(n):
    return f'00000000-0000-4000-8000-{n:012x}'

cards, queries = [], []
for i, (ru, en, category, code) in enumerate(TOPICS):
    ids = [rid(100 + i * 4 + j) for j in range(4)]
    cat = rid(1000 + i)
    titles = [f'{ru.capitalize()}: {code}', f'{en.capitalize()} guide',
              f'{ru.capitalize()} — проверка', f'{en.capitalize()} reference']
    descriptions = [f'Синтетический пример {ru}. Точный идентификатор {code}.',
                    f'Synthetic {en} workflow and {category} documentation.',
                    f'Ручная карточка: {ru}. Канон остаётся сохранным.',
                    f'Literal {en} checklist, distinct from the guide.']
    for j in range(4):
        cards.append(dict(record_id=ids[j], title=titles[j], description=descriptions[j],
                          category_id=cat if j == 1 else None, category_label=category if j == 1 else None,
                          source_kind='imported_image', role=['exact-title','category-guide','ru-check','en-reference'][j]))
    split = 'tuning' if i < 6 else 'heldout'
    cases = [(ru, ids[::2], None, 'literal-ru'), (en, ids[1::2], None, 'literal-en'),
             (code, [ids[0]], None, 'technical-exact'), (f'{ru} {en}', ids, None, 'any-terms-or'),
             (en, [ids[1]], cat, 'rare-category-before-topk')]
    for j, (text, relevant, category_id, group) in enumerate(cases):
        queries.append(dict(id=f'positive-{i:02}-{j}', text=text, split=split, group=group,
                            category_id=category_id, relevance={r: 3 for r in relevant},
                            forbidden_ids=[r for r in ids if r not in relevant],
                            filter_role='stable-category-id' if category_id else 'any',
                            expected_top1=ids[0] if group == 'technical-exact' else None))
    # Near-looking but nonidentical literal words: no stemming/translation claim.
    queries.append(dict(id=f'negative-{i:02}', text=f'AbsentMarker{i:02}Z', split=split,
                        group='negative', category_id=None, relevance={}, forbidden_ids=ids,
                        filter_role='any', expected_top1=None))
# Distinct plausible distractors; no gold relevance to invented synonyms.
for i in range(20):
    cards.append(dict(record_id=rid(500+i), title=f'Synthetic unrelated photograph {i}',
                      description='Seaside trees and clouds. Синтетический пейзаж у моря.',
                      category_id=None, category_label=None, source_kind='imported_image', role='distractor'))
value = dict(version='production_anyterms_exact_gold_v1', status='declared before tuning',
             configuration='seven_fields_6_3_1_2_2_2_1_win_nfc_unicase2.9_v1',
             semantics='quoted OR <=64 atoms; no stemming, translation or semantic inference',
             cards=cards, queries=queries,
             quality_target={'macro_recall_at_5': .85, 'scope_filter_violations': 0, 'negative_false_positives': 0})
with pathlib.Path(sys.argv[1]).open('x', encoding='utf-8', newline='\n') as f:
    json.dump(value, f, ensure_ascii=False, indent=2)
    f.write('\n')
