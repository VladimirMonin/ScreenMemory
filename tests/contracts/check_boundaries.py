"""S01-DEP-01: explicit workspace/package dependency allowlist; no future crates/adapters."""
from pathlib import Path
import json
import tomllib

root = Path(__file__).resolve().parents[2]
workspace = tomllib.loads((root / 'Cargo.toml').read_text(encoding='utf-8'))['workspace']
assert set(workspace['members']) == {'crates/domain', 'crates/protocol', 'crates/application', 'crates/observability', 'crates/isolation', 'crates/storage', 'crates/credentials', 'crates/framing', 'crates/pipe', 'crates/session', 'crates/host', 'crates/cli', 'crates/gui'}
allowed = {
    'domain': {'serde', 'uuid', 'schemars'},
    'protocol': {'sm-domain', 'serde', 'serde_json', 'schemars', 'time'},
    'application': {'sm-domain', 'sm-protocol', 'async-trait', 'tokio-util', 'serde', 'uuid', 'getrandom', 'sha2', 'subtle', 'zeroize'},
    'observability': {'sm-application', 'serde', 'serde_json'},
    'isolation': {'sm-application'},
    'storage': {'sm-application', 'sm-isolation', 'async-trait', 'rusqlite', 'sha2', 'serde_json', 'uuid', 'tokio'},
    'credentials': {'sm-application', 'sm-isolation', 'windows-sys', 'zeroize'},
    'framing': {'serde', 'serde_json', 'tokio', 'zeroize'},
    'pipe': {'sm-isolation', 'sha2', 'tokio', 'windows-sys'},
    'session': {'sm-application', 'sm-domain', 'sm-protocol', 'sm-framing', 'sm-pipe', 'serde', 'serde_json', 'uuid', 'tokio-util', 'tokio'},
    'host': {'sm-application', 'sm-credentials', 'sm-domain', 'sm-framing', 'sm-isolation', 'sm-observability', 'sm-pipe', 'sm-protocol', 'sm-session', 'sm-storage', 'async-trait', 'serde', 'serde_json', 'tokio', 'tokio-util', 'uuid'},
    'cli': {'sm-application', 'sm-credentials', 'sm-domain', 'sm-framing', 'sm-isolation', 'sm-pipe', 'sm-protocol', 'sm-session', 'serde', 'serde_json', 'tokio', 'windows-sys'},
    'gui': {'sm-application', 'sm-cli', 'sm-credentials', 'sm-domain', 'sm-isolation', 'sm-protocol', 'sm-session', 'serde', 'serde_json', 'tauri', 'tokio', 'windows-sys'},
}
for crate, expected in allowed.items():
    manifest = tomllib.loads((root / 'crates' / crate / 'Cargo.toml').read_text(encoding='utf-8'))
    assert set(manifest['dependencies']) == expected, crate
    assert manifest['package']['name'] == 'sm-' + crate
    if crate == 'isolation':
        assert set(manifest['dev-dependencies']) == {'sm-observability', 'uuid'}
    if crate == 'pipe':
        assert set(manifest['dev-dependencies']) == {'sm-framing', 'serde_json', 'tokio'}
    if crate == 'session':
        assert set(manifest['dev-dependencies']) == {'async-trait', 'sm-isolation'}
    if crate == 'host':
        assert 'dev-dependencies' not in manifest, 'host composition tests use the production ports only'
    if crate == 'cli':
        assert set(manifest['dev-dependencies']) == {'sm-host', 'tokio-util', 'uuid'}
    if crate == 'gui':
        assert set(manifest['dev-dependencies']) == {'sm-framing', 'sm-host', 'sm-pipe', 'tokio-util', 'uuid'}
        assert manifest['bin'][0]['name'] == 'screenmemory-gui'
    for source in (root / 'crates' / crate / 'src').rglob('*.rs'):
        text = source.read_text(encoding='utf-8')
        forbidden_modules = ('tauri::', 'winapi::', 'lmstudio::', 'std::net::') if crate == 'storage' else ('tauri::', 'rusqlite::', 'winapi::', 'lmstudio::', 'std::net::')
        if crate == 'gui':
            # The window speaks only the shared client; it never opens storage or the wire.
            forbidden_modules = ('rusqlite::', 'sm_storage::', 'winapi::', 'lmstudio::', 'std::net::')
        for forbidden in forbidden_modules:
            assert forbidden not in text, (crate, source.name, forbidden)
        if crate not in ('observability', 'isolation', 'storage', 'credentials', 'pipe', 'session', 'host', 'cli', 'gui'):
            for forbidden in ('windows::', 'std::fs::'):
                assert forbidden not in text, (crate, source.name, forbidden)
# S01-T05d boundaries: the CLI never opens the library database, never links the storage
# owner and can never name a host image other than the configured trusted executable.
cli_sources = {source.name: source.read_text(encoding='utf-8') for source in (root / 'crates/cli/src').rglob('*.rs')}
for name, text in cli_sources.items():
    for forbidden in ('sm_storage::', 'rusqlite::', 'Connection::open', 'sqlite'):
        assert forbidden not in text, ('cli', name, forbidden)
assert set(cli_sources) == {'args.rs', 'config.rs', 'exec.rs', 'launcher.rs', 'lib.rs', 'main.rs', 'output.rs'}
assert 'Command::new' in cli_sources['launcher.rs']
for name, text in cli_sources.items():
    if name != 'launcher.rs':
        assert 'Command::new' not in text, ('cli', name)
host_sources = {source.name: source.read_text(encoding='utf-8') for source in (root / 'crates/host/src').rglob('*.rs')}
for name, text in host_sources.items():
    for forbidden in ('Command::new', 'std::net::', 'tauri::', 'lmstudio::'):
        assert forbidden not in text, ('host', name, forbidden)
for crate, binary in (('host', 'shotd'), ('cli', 'shot')):
    manifest = tomllib.loads((root / 'crates' / crate / 'Cargo.toml').read_text(encoding='utf-8'))
    assert manifest['bin'][0]['name'] == binary, crate
manifest = json.loads((root / 'tests/fixtures/s01/root-isolation-manifest.json').read_text(encoding='utf-8'))
assert manifest['schema_version'] == 1
assert manifest['fixture_scope'].startswith('.pi/temporary/')
assert len(manifest['required_tests']) == len(set(manifest['required_tests'])) == 9
native_tests = (root / 'crates/isolation/tests/root_isolation.rs').read_text(encoding='utf-8')
native_unit = (root / 'crates/isolation/src/windows.rs').read_text(encoding='utf-8')
for test_id in manifest['required_tests']:
    assert f'fn {test_id}(' in (native_tests + native_unit), test_id
policy = (root / 'crates/application/src/credentials.rs').read_text(encoding='utf-8')
native_credentials = (root / 'crates/credentials/src/lib.rs').read_text(encoding='utf-8')
assert 'self.verifier.ct_eq(&digest)' in policy and 'saved.as_bytes().ct_eq(generated.as_bytes())' in policy
assert 'pub struct Secret(Vec<u8>);' in policy and 'self.0.zeroize()' in policy
assert 'CredReadW(' in native_credentials and 'CredWriteW(' in native_credentials and 'CredFree(' in native_credentials
assert 'CredEnumerate' not in native_credentials and 'std::env::var(' not in native_credentials
assert 'windows_sys::' not in policy and 'windows_sys::' not in (root / 'crates/application/src/lib.rs').read_text(encoding='utf-8')
storage_manifest = tomllib.loads((root / 'crates/storage/Cargo.toml').read_text(encoding='utf-8'))
assert storage_manifest['dependencies']['rusqlite']['features'] == ['bundled']
assert storage_manifest['dependencies']['rusqlite']['version'].startswith('=')
assert 'rusqlite::' not in (root / 'crates/application/src/storage.rs').read_text(encoding='utf-8')
# S01-T06d boundaries: the diagnostic window is one client of the shared session owner.
gui_sources = {source.name: source.read_text(encoding='utf-8') for source in (root / 'crates/gui/src').rglob('*.rs')}
assert set(gui_sources) == {'config.rs', 'lib.rs', 'main.rs', 'model.rs', 'open.rs', 'selfcheck.rs', 'session.rs', 'shell.rs'}
assert 'ApplicationClient' in gui_sources['session.rs'] and 'sm_session' in gui_sources['session.rs']
assert 'connect_session' in gui_sources['session.rs'], 'the window must reuse the shared client integration path'
assert 'ApplicationClient' not in gui_sources['shell.rs'], 'handlers must not bypass the session owner'
for name, text in gui_sources.items():
    assert 'sm_sqlite' not in text and 'Connection::open' not in text
    if name != 'open.rs':
        assert 'Command::new' not in text, ('gui', name)
        assert 'std::fs::' not in text, ('gui', name, 'std::fs::')
        assert 'std::os::windows' not in text, ('gui', name, 'std::os::windows')
        assert 'windows_sys::' not in text or name == 'main.rs', ('gui', name, 'windows_sys::')
# Only the report opener may start a shell, and it must come from the Windows directory.
assert 'GetWindowsDirectoryW' in gui_sources['open.rs']
assert 'std::os::windows::fs::MetadataExt' in gui_sources['open.rs']
assert 'std::env::var' not in gui_sources['open.rs'] and 'explorer' in gui_sources['open.rs']
# No implicit root/configuration: the only accepted argument is an absolute local --config.
assert '--config' in gui_sources['config.rs'] and 'std::env::var' not in gui_sources['config.rs']
assert 'load_config' in gui_sources['config.rs'] and '"--self-check"' in gui_sources['config.rs']
gui_config = json.loads((root / 'crates/gui/tauri.conf.json').read_text(encoding='utf-8'))
assert gui_config['identifier'].startswith('test.'), 'the shell stays in the explicit test identity'
assert gui_config['bundle']['active'] is False, 'no production bundle/installer in this slice'
assert gui_config['build']['frontendDist'] == '../../apps/gui/dist'
assert gui_config['app']['windows'][0]['create'] is False, 'the window is built by the shell, not at startup'
capabilities = json.loads((root / 'crates/gui/capabilities/default.json').read_text(encoding='utf-8'))
assert capabilities['permissions'] == ['core:default'], 'no plugin permission is added to the diagnostic window'
gui_package = json.loads((root / 'apps/gui/package.json').read_text(encoding='utf-8'))
assert set(gui_package['dependencies']) == {'@tauri-apps/api', 'react', 'react-dom'}
assert set(gui_package['devDependencies']) == {'@types/react', '@types/react-dom', 'typescript', 'vite', 'vitest'}
frontend = {source.name: source.read_text(encoding='utf-8') for source in (root / 'apps/gui/src').glob('*') if source.suffix in ('.ts', '.tsx')}
assert set(frontend) == {'App.tsx', 'main.tsx', 'model.ts', 'model.test.ts'}
for name, text in frontend.items():
    for forbidden in ('@lmstudio/sdk', 'better-sqlite3', 'sqlite', 'child_process', 'XMLHttpRequest'):
        assert forbidden not in text, ('gui-frontend', name, forbidden)
print('S01-DEP-01 PASS: thirteen crates, exact dependency allowlists, bundled SQLite in storage and native credentials in adapter; host/CLI composition and the diagnostic-window client boundary retained')
