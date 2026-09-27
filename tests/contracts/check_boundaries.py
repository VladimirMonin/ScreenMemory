"""S01-DEP-01: explicit workspace/package dependency allowlist; no future crates/adapters."""
from pathlib import Path
import tomllib

root = Path(__file__).resolve().parents[2]
workspace = tomllib.loads((root / 'Cargo.toml').read_text(encoding='utf-8'))['workspace']
assert set(workspace['members']) == {'crates/domain', 'crates/protocol', 'crates/application', 'crates/observability'}
allowed = {
    'domain': {'serde', 'uuid', 'schemars'},
    'protocol': {'sm-domain', 'serde', 'serde_json', 'schemars', 'time'},
    'application': {'sm-domain', 'sm-protocol', 'async-trait', 'tokio-util', 'serde'},
    'observability': {'sm-application', 'serde', 'serde_json'},
}
for crate, expected in allowed.items():
    manifest = tomllib.loads((root / 'crates' / crate / 'Cargo.toml').read_text(encoding='utf-8'))
    assert set(manifest['dependencies']) == expected, crate
    assert manifest['package']['name'] == 'sm-' + crate
    for source in (root / 'crates' / crate / 'src').rglob('*.rs'):
        text = source.read_text(encoding='utf-8')
        for forbidden in ('tauri::', 'rusqlite::', 'winapi::', 'lmstudio::', 'std::net::'):
            assert forbidden not in text, (crate, source.name, forbidden)
        if crate != 'observability':
            for forbidden in ('windows::', 'std::fs::'):
                assert forbidden not in text, (crate, source.name, forbidden)
print('S01-DEP-01 PASS: four crates, exact direct dependency allowlists, no concrete OS/DB/SDK imports in foundation owners')
