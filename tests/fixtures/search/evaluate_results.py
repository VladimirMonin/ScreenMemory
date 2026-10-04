"""Deterministic metrics from frozen relevance and actual shared-service IDs; no tuning."""
import hashlib
import json
import math
import pathlib
import sys

gold_path, actual_path, output_path = map(pathlib.Path, sys.argv[1:])
gold = json.loads(gold_path.read_text(encoding='utf-8'))
actual = json.loads(actual_path.read_text(encoding='utf-8'))
by_id = {q['query_id']: q for q in actual['queries']}
assert set(by_id) == {q['id'] for q in gold['queries']}
groups = {}
violations = []
for q in gold['queries']:
    ids = by_id[q['id']]['actual_ids']
    relevance = q['relevance']
    if len(ids) != len(set(ids)):
        violations.append(q['id'] + ': duplicate')
    if set(ids) & set(q['forbidden_ids']):
        violations.append(q['id'] + ': forbidden scope/filter/source role')
    dcg = sum((2 ** relevance.get(r, 0) - 1) / math.log2(i + 2) for i, r in enumerate(ids[:5]))
    ideal = sum((2 ** grade - 1) / math.log2(i + 2) for i, grade in enumerate(sorted(relevance.values(), reverse=True)[:5]))
    metrics = {'queries': 1}
    if relevance:
        metrics.update(positive=1, recall_at_5=len(set(ids[:5]) & set(relevance)) / len(relevance),
                       hit_at_5=int(bool(set(ids[:5]) & set(relevance))),
                       mrr_at_10=next((1 / (i + 1) for i, r in enumerate(ids[:10]) if r in relevance), 0),
                       ndcg_at_5=dcg / ideal)
    else:
        metrics.update(negative=1, false_positives=len(ids))
    if q['expected_top1'] is not None:
        metrics.update(exact=1, exact_top1=int(bool(ids) and ids[0] == q['expected_top1']))
    for name in ['all', 'split:' + q['split'], 'group:' + q['group']]:
        group = groups.setdefault(name, {})
        for key, value in metrics.items():
            group[key] = group.get(key, 0) + value
for group in groups.values():
    for metric in ['recall_at_5', 'hit_at_5', 'mrr_at_10', 'ndcg_at_5']:
        if group.get('positive'):
            group[metric] /= group['positive']
    if group.get('exact'):
        group['exact_top1'] /= group['exact']
report = dict(gold_sha256=hashlib.sha256(gold_path.read_bytes()).hexdigest(),
              actual_sha256=hashlib.sha256(actual_path.read_bytes()).hexdigest(),
              groups=groups, strict_scope_filter_violations=violations,
              limits='literal RU/EN only; no translation/stemming/semantic claim; complete graded relevance retained')
with output_path.open('x', encoding='utf-8', newline='\n') as f:
    json.dump(report, f, ensure_ascii=False, indent=2)
    f.write('\n')
assert not violations
assert groups['all']['recall_at_5'] >= .85
assert groups['all']['false_positives'] == 0
print(json.dumps(groups['all']))
