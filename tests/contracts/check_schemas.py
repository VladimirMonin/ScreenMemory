"""S01-T03a draft 2020-12 / synthetic cross-language fixture gate.
Requires jsonschema 4.26.0 in the caller's isolated Python environment; no auto-install.
"""
import copy
import json
from importlib.metadata import version
from pathlib import Path

assert version('jsonschema') == '4.26.0', 'explicit verifier version required'
from jsonschema import Draft202012Validator  # noqa: E402

root = Path(__file__).resolve().parents[2]
schemas = {p.stem: json.loads(p.read_text(encoding='utf-8')) for p in (root / 'schemas').glob('*.schema.json')}
assert len(schemas) == 4
for name, schema in schemas.items():
    Draft202012Validator.check_schema(schema)
    assert schema['$schema'] == 'https://json-schema.org/draft/2020-12/schema', name

app = Draft202012Validator(schemas['application-v1.schema'])
methods = Draft202012Validator(schemas['application-methods-v1.schema'])
errors = Draft202012Validator(schemas['foundation-errors.schema'])
valid = json.loads((root / 'tests/fixtures/protocol/valid-status.json').read_text(encoding='utf-8'))
control = json.loads((root / 'tests/fixtures/protocol/valid-control.json').read_text(encoding='utf-8'))
for request in (valid, control):
    app.validate(request)
    methods.validate({'method': request['method'], 'params': request['params']})
for code in ('METHOD_NOT_SUPPORTED', 'INVALID_REQUEST', 'PROTOCOL_MISMATCH'):
    errors.validate(code)
for code in ('GUESSED_CODE', 'SUCCESS'):
    assert not errors.is_valid(code)

def bad(value, schema=app):
    assert not schema.is_valid(value), value

bad(dict(valid, client_kind='gui'))
bad(dict(valid, library_id='not-a-uuid'))
bad({'method': 'system.status', 'params': {'client_kind': 'gui'}}, methods)
bad({'method': 'system.set_ai_policy', 'params': {'expected_revision': 9007199254740993, 'enabled': True}}, methods)
bad({'method': 'system.set_ai_policy', 'params': {'expected_revision': '18446744073709551616', 'enabled': True}}, methods)
for seconds in (0, 301):
    bad({'method': 'system.authorize_probe', 'params': {'fixture': 'synthetic_menu_v1', 'profile_id': 'x', 'allow_load': False, 'lifetime_seconds': seconds}}, methods)
result = {'request_id': valid['request_id'], 'kind': 'result', 'result': {'type': 'ai_policy', 'value': {'revision': '1', 'enabled': False}}}
error = {'request_id': valid['request_id'], 'kind': 'error', 'error': {'code': 'INVALID_REQUEST', 'safe_message': 'Invalid.', 'retryable': False}}
app.validate(result)
app.validate(error)
# S01-SCHEMA-02: shared Rust/schema fixtures, including a terminal LF (where
# regex $ anchors can disagree across engines) and Unicode outside C0/DEL/C1.
safe_cases = json.loads((root / 'tests/fixtures/protocol/safe-messages.json').read_text(encoding='utf-8'))
for case in safe_cases['invalid']:
    frame = copy.deepcopy(error)
    frame['error']['safe_message'] = case['safe_message']
    assert not app.is_valid(frame), case['id']
for case in safe_cases['valid']:
    frame = copy.deepcopy(error)
    frame['error']['safe_message'] = case['safe_message']
    app.validate(frame)
# In-memory mutation proves the negative fixtures really detect this guard's
# absence without touching the Rust owner or a generated file.
without_guard = copy.deepcopy(schemas['application-v1.schema'])
assert without_guard['$defs']['SafeMessage'].pop('not') == {'pattern': r'[\u0000-\u001F\u007F-\u009F]'}
unguarded_app = Draft202012Validator(without_guard)
for case in safe_cases['invalid']:
    frame = copy.deepcopy(error)
    frame['error']['safe_message'] = case['safe_message']
    assert unguarded_app.is_valid(frame), case['id']
for name in ('application-v1.schema', 'bridge-v1.schema'):
    defs = schemas[name]['$defs']
    check = defs['CheckResult']
    assert 'duration_ms' not in check.get('required', [])
    assert check['properties']['duration_ms']['type'] == 'integer'
    validator = Draft202012Validator({'$schema': schemas[name]['$schema'], '$defs': defs, '$ref': '#/$defs/CheckResult'})
    assert validator.is_valid({'check_id': 'fixture', 'outcome': 'NOT_RUN'})
    assert not validator.is_valid({'check_id': 'fixture', 'outcome': 'NOT_RUN', 'duration_ms': None})
bad(dict(result, error=error['error']))
bad(dict(error, result=result['result']))
print('S01-SCHEMA-01/02 PASS: 4 draft2020-12 schemas; shared control/Unicode fixtures and in-memory guard-removal regression; union/revision/consent/identifier constraints')
