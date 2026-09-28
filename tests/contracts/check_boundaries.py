"""S01-DEP-01: explicit workspace/package dependency allowlist; no future crates/adapters."""
from pathlib import Path
import json
import tomllib

root = Path(__file__).resolve().parents[2]
workspace = tomllib.loads((root / 'Cargo.toml').read_text(encoding='utf-8'))['workspace']
assert set(workspace['members']) == {'crates/domain', 'crates/protocol', 'crates/application', 'crates/observability', 'crates/isolation', 'crates/storage'}
allowed = {
    'domain': {'serde', 'uuid', 'schemars'},
    'protocol': {'sm-domain', 'serde', 'serde_json', 'schemars', 'time'},
    'application': {'sm-domain', 'sm-protocol', 'async-trait', 'tokio-util', 'serde', 'uuid'},
    'observability': {'sm-application', 'serde', 'serde_json'},
    'isolation': {'sm-application'},
    'storage': {'sm-application', 'sm-isolation', 'async-trait', 'rusqlite', 'sha2', 'serde_json', 'uuid', 'tokio'},
}
for crate, expected in allowed.items():
    manifest = tomllib.loads((root / 'crates' / crate / 'Cargo.toml').read_text(encoding='utf-8'))
    assert set(manifest['dependencies']) == expected, crate
    assert manifest['package']['name'] == 'sm-' + crate
    if crate == 'isolation':
        assert set(manifest['dev-dependencies']) == {'sm-observability'}
    for source in (root / 'crates' / crate / 'src').rglob('*.rs'):
        text = source.read_text(encoding='utf-8')
        for forbidden in (('tauri::', 'winapi::', 'lmstudio::', 'std::net::') if crate == 'storage' else ('tauri::', 'rusqlite::', 'winapi::', 'lmstudio::', 'std::net::')):
            assert forbidden not in text, (crate, source.name, forbidden)
        if crate not in ('observability', 'isolation', 'storage'):
            for forbidden in ('windows::', 'std::fs::'):
                assert forbidden not in text, (crate, source.name, forbidden)
manifest = json.loads((root / 'tests/fixtures/s01/root-isolation-manifest.json').read_text(encoding='utf-8'))
assert manifest['schema_version'] == 1
assert manifest['fixture_scope'].startswith('.pi/temporary/')
assert len(manifest['required_tests']) == len(set(manifest['required_tests'])) == 9
native_tests = (root / 'crates/isolation/tests/root_isolation.rs').read_text(encoding='utf-8')
native_unit = (root / 'crates/isolation/src/windows.rs').read_text(encoding='utf-8')
for test_id in manifest['required_tests']:
    assert f'fn {test_id}(' in (native_tests + native_unit), test_id
storage_manifest = tomllib.loads((root / 'crates/storage/Cargo.toml').read_text(encoding='utf-8'))
assert storage_manifest['dependencies']['rusqlite']['features'] == ['bundled']
assert storage_manifest['dependencies']['rusqlite']['version'].startswith('=')
assert 'rusqlite::' not in (root / 'crates/application/src/storage.rs').read_text(encoding='utf-8')
print('S01-DEP-01 PASS: six crates, exact dependency allowlists, bundled SQLite isolated in storage; native root boundary retained')
