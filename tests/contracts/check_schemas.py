"""S01-T03a draft 2020-12 / synthetic cross-language fixture gate.
Requires jsonschema 4.26.0 in the caller's isolated Python environment; no auto-install.
"""
import copy
import json
import re
from calendar import monthrange
from importlib.metadata import version
from pathlib import Path

assert version('jsonschema') == '4.26.0', 'explicit verifier version required'
from jsonschema import Draft202012Validator, FormatChecker  # noqa: E402

# jsonschema's default date-time checker accepts February 29 in non-leap years.
# This dev-only adapter checks RFC3339's calendar and UTC-Z syntax for the
# one current date-time wire field; the Rust owner and generated schema stay unchanged.
formats = FormatChecker()
UTC_DATE_TIME = re.compile(
    r'([0-9]{4})-(0[1-9]|1[0-2])-([0-9]{2})T'
    r'([0-9]{2}):([0-9]{2}):([0-9]{2})(?:\.[0-9]+)?Z'
)


@formats.checks('date-time')
def strict_utc_date_time(value):
    if not isinstance(value, str):
        return True  # JSON Schema's type keyword owns non-strings.
    match = UTC_DATE_TIME.fullmatch(value)
    if match is None:
        return False
    year, month, day, hour, minute, second = map(int, match.groups())
    # calendar supports the four-digit year 0000; no extra year-range policy.
    return (1 <= day <= monthrange(year, month)[1]
            and hour <= 23 and minute <= 59 and second <= 59)

@formats.checks('uint64')
def strict_uint64(value):
    if not isinstance(value, (int, float)):
        return True  # type keyword owns non-numbers.
    return value == int(value) and 0 <= value < 2 ** 64


root = Path(__file__).resolve().parents[2]
schemas = {p.stem: json.loads(p.read_text(encoding='utf-8')) for p in (root / 'schemas').glob('*.schema.json')}
assert len(schemas) == 4
for name, schema in schemas.items():
    Draft202012Validator.check_schema(schema)
    assert schema['$schema'] == 'https://json-schema.org/draft/2020-12/schema', name

app = Draft202012Validator(schemas['application-v1.schema'], format_checker=formats)
methods = Draft202012Validator(schemas['application-methods-v1.schema'])
errors = Draft202012Validator(schemas['foundation-errors.schema'])
valid = json.loads((root / 'tests/fixtures/protocol/valid-status.json').read_text(encoding='utf-8'))
control = json.loads((root / 'tests/fixtures/protocol/valid-control.json').read_text(encoding='utf-8'))
for request in (valid, control):
    app.validate(request)
    methods.validate({'method': request['method'], 'params': request['params']})
# Same synthetic request mutations are decoded by Rust and by the TS/Ajv consumer.
parity = json.loads((root / 'tests/fixtures/protocol/parity-cases.json').read_text(encoding='utf-8'))
for group, expected in (('valid', True), ('invalid', False)):
    for case in parity[group]:
        request = copy.deepcopy(control if case['base'] == 'control' else valid)
        request.update({key: value for key, value in case.items() if key not in ('base', 'id')})
        actual = app.is_valid(request) and methods.is_valid({'method': request['method'], 'params': request['params']})
        assert actual == expected, case['id']
# The same complete response frames are decoded by Rust, Ajv and this verifier.
responses = json.loads((root / 'tests/fixtures/protocol/response-cases.json').read_text(encoding='utf-8'))
for group, expected in (('valid', True), ('invalid', False)):
    for case in responses[group]:
        assert app.is_valid(case['frame']) == expected, case['id']
# S04 execution DTOs remain metadata, never deserializable instance/claim authority.
access = json.loads((root / 'tests/fixtures/protocol/access-cases.json').read_text(encoding='utf-8'))
for case in access:
    assert methods.is_valid(case['value']) == case['valid'], case['id']
base_access = next(c['value'] for c in access if c['id'] == 'create')
for length, valid_name in [(0, False), (1, True), (80, True), (81, False)]:
    value = copy.deepcopy(base_access)
    value['params']['name'] = '😀' * length
    assert methods.is_valid(value) == valid_name
knowledge = json.loads((root / 'tests/fixtures/protocol/knowledge-cases.json').read_text(encoding='utf-8'))
for case in knowledge:
    assert methods.is_valid(case['value']) == case['valid'], case['id']
processing = json.loads((root / 'tests/fixtures/protocol/processing-cases.json').read_text(encoding='utf-8'))
for case in processing:
    assert methods.is_valid(case['value']) == case['valid']
execution = json.loads((root / 'tests/fixtures/protocol/processing-execution-responses.json').read_text(encoding='utf-8'))
for case in execution:
    assert app.is_valid(case['frame']) == case['valid']
    for tokens in case.get('reject_generated_tokens', []):
        frame = copy.deepcopy(case['frame'])
        frame['result']['value']['value']['probe_result']['generated_tokens'] = tokens
        assert not app.is_valid(frame)
# Mutation proof: without the strict calendar checker, the ordinary shared
# negative fixture is erroneously accepted (without modifying source/schema).
calendar_case = next(c['frame'] for c in responses['invalid'] if c['id'] == 'consent-not-leap-year')
assert not app.is_valid(calendar_case)
assert Draft202012Validator(schemas['application-v1.schema']).is_valid(calendar_case)
without_response_guard = copy.deepcopy(schemas['application-v1.schema'])
assert without_response_guard['$defs']['Response']['oneOf'][0]['additionalProperties'] is False
without_response_guard['$defs']['Response']['oneOf'][0]['additionalProperties'] = True
both_case = next(c['frame'] for c in responses['invalid'] if c['id'] == 'both-on-result')
assert not app.is_valid(both_case)
assert Draft202012Validator(without_response_guard, format_checker=formats).is_valid(both_case)
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
print('S01-SCHEMA-01/02/03 PASS: 4 draft2020-12 schemas; shared request/response/SafeMessage fixtures; strict UTC calendar and in-memory guard-removal regressions')
