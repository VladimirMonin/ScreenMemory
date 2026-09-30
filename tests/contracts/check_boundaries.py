"""S01-DEP-01: explicit workspace/package dependency allowlist; no future crates/adapters."""
from pathlib import Path
import json
import tomllib

root = Path(__file__).resolve().parents[2]
workspace = tomllib.loads((root / 'Cargo.toml').read_text(encoding='utf-8'))['workspace']
assert set(workspace['members']) == {'crates/domain', 'crates/protocol', 'crates/application', 'crates/observability', 'crates/isolation', 'crates/storage', 'crates/codec', 'crates/credentials', 'crates/framing', 'crates/pipe', 'crates/capture', 'crates/session', 'crates/host', 'crates/cli', 'crates/gui'}
allowed = {
    'domain': {'serde', 'uuid', 'schemars'},
    'protocol': {'sm-domain', 'serde', 'serde_json', 'schemars', 'time'},
    'application': {'sm-domain', 'sm-protocol', 'async-trait', 'tokio-util', 'serde', 'serde_json', 'uuid', 'getrandom', 'sha2', 'subtle', 'zeroize'},
    'observability': {'sm-application', 'serde', 'serde_json'},
    'isolation': {'sm-application', 'sm-domain', 'uuid'},
    'storage': {'sm-application', 'sm-domain', 'sm-isolation', 'async-trait', 'rusqlite', 'sha2', 'serde', 'serde_json', 'uuid', 'tokio'},
    'codec': {'libwebp-sys'},
    'credentials': {'sm-application', 'sm-isolation', 'windows-sys', 'zeroize'},
    'framing': {'serde', 'serde_json', 'tokio', 'zeroize'},
    'pipe': {'sm-isolation', 'sha2', 'tokio', 'windows-sys'},
    'capture': {'sm-application', 'uuid', 'windows-sys'},
    'session': {'sm-application', 'sm-domain', 'sm-protocol', 'sm-framing', 'sm-pipe', 'serde', 'serde_json', 'uuid', 'tokio-util', 'tokio'},
    'host': {'sm-application', 'sm-capture', 'sm-codec', 'sm-credentials', 'sm-domain', 'sm-framing', 'sm-isolation', 'sm-observability', 'sm-pipe', 'sm-protocol', 'sm-session', 'sm-storage', 'async-trait', 'serde', 'serde_json', 'tokio', 'tokio-util', 'uuid'},
    'cli': {'sm-application', 'sm-credentials', 'sm-domain', 'sm-framing', 'sm-isolation', 'sm-pipe', 'sm-protocol', 'sm-session', 'serde', 'serde_json', 'tokio', 'uuid', 'windows-sys'},
    'gui': {'sm-application', 'sm-cli', 'sm-credentials', 'sm-domain', 'sm-isolation', 'sm-protocol', 'sm-session', 'serde', 'serde_json', 'tauri', 'tokio', 'uuid', 'windows-sys'},
}
for crate, expected in allowed.items():
    manifest = tomllib.loads((root / 'crates' / crate / 'Cargo.toml').read_text(encoding='utf-8'))
    assert set(manifest['dependencies']) == expected, crate
    assert manifest['package']['name'] == 'sm-' + crate
    if crate == 'storage':
        assert set(manifest['dev-dependencies']) == {'sm-codec'}
    if crate == 'codec':
        assert 'dev-dependencies' not in manifest
        assert manifest['dependencies']['libwebp-sys']['version'] == '=0.14.4'
        assert manifest['dependencies']['libwebp-sys']['default-features'] is False
        assert manifest['dependencies']['libwebp-sys']['features'] == ['std']
    if crate == 'isolation':
        assert set(manifest['dev-dependencies']) == {'sm-observability', 'uuid'}
    if crate == 'pipe':
        assert set(manifest['dev-dependencies']) == {'sm-framing', 'serde_json', 'tokio'}
    if crate == 'capture':
        # The live native suite posts messages to its own overlay window, so the audited
        # Windows bindings are needed as a dev-dependency as well; the production surface
        # is unchanged.
        assert set(manifest['dev-dependencies']) == {'windows-sys'}
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
        if crate == 'capture':
            # The native owner reads pixels and windows only: no storage, no codec, no
            # filesystem, no environment, no network and no log sink of its own.
            forbidden_modules = ('tauri::', 'rusqlite::', 'sm_storage::', 'sm_codec::', 'std::fs::', 'std::net::', 'std::env::var', 'lmstudio::', 'winapi::', 'println!', 'eprintln!', 'log::', 'tracing::')
        if crate == 'gui':
            # The window speaks only the shared client; it never opens storage or the wire.
            forbidden_modules = ('rusqlite::', 'sm_storage::', 'winapi::', 'lmstudio::', 'std::net::')
        if crate == 'codec':
            # The codec is pure in-memory pixel work: no storage, no filesystem, no
            # environment, no network and no log sink of its own.
            forbidden_modules = ('rusqlite::', 'sm_storage::', 'std::fs::', 'std::net::', 'std::env::', 'winapi::', 'windows::', 'tauri::', 'lmstudio::', 'println!', 'eprintln!', 'log::', 'tracing::')
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
assert 'std::fs::OpenOptions' in cli_sources['exec.rs'], 'the capture copy writes only its explicit --output file'
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
# S02 images storage: the owner keeps its single writer, the canonical file layout and
# the closed canonical-format vocabulary; the codec never reaches the filesystem.
storage_sources = {source.name: source.read_text(encoding='utf-8') for source in (root / 'crates/storage/src').rglob('*.rs')}
assert set(storage_sources) == {'lib.rs', 'migration.rs', 'images.rs', 'staging.rs', 'staging_files.rs', 'staging_tests.rs', 'staging_recovery_tests.rs', 'staging_files_tests.rs', 'file_lane_tests.rs', 'tests.rs', 'image_tests.rs'}
# S02-T09b staging validator: pure schema decoding only. It must not reach a filesystem,
# a SQLite connection, a process or the network, and it never publishes a path or a
# durable-input claim from manifest content.
for forbidden in ('std::fs', 'std::path', 'PathBuf', 'rusqlite', 'std::process', 'std::net', 'durable_input'):
    assert forbidden not in storage_sources['staging.rs'], ('storage', 'staging.rs', forbidden)
for member in ('schema_version', 'kind', 'state', 'library_id', 'principal_id', 'operation_id',
               'record_id', 'asset_id', 'frame_id', 'mode', 'width', 'height', 'stride',
               'byte_length', 'pixels_sha256', 'context_sha256', 'encoder_fingerprint', 'captured_at'):
    assert f'"{member}"' in storage_sources['staging.rs'], ('staging manifest member', member)
assert 'use sm_application::' in storage_sources['staging.rs'] and \
    'capture::{MAX_CAPTURE_PIXELS, MAX_CAPTURE_SIDE}' in storage_sources['staging.rs'], \
    'the staging ceilings come from the capture seam, never from a second local constant'
assert storage_sources['staging.rs'].count('MAX_CAPTURE_SIDE') == 3, \
    'the side ceiling is only imported and applied, never redefined'
assert 'is_canonical_utc_timestamp' in storage_sources['staging.rs']
assert 'CaptureContext::from_json' in storage_sources['staging.rs']
# S02-T09h stays pure. V1 retains its exact 18-field decoder; v2 is versioned and
# receipt-ready only with validated warnings, automatic provenance and fixed policy.
# T09j connects the owned v2 request to the existing file lane and host. T09k adds
# known-operation v2 reopen. T09l2 adds bounded discovery only on the file lane;
# there is still no SQL recovery or startup enumeration.
assert 'const MANIFEST_FIELDS: [&str; 18]' in storage_sources['staging.rs']
for member in ('"warnings"', '"provenance_kind"', '"path_policy"'):
    assert member in storage_sources['staging.rs']
assert 'warnings_from_json' in storage_sources['staging.rs']
assert 'RECOVERY_PATH_POLICY' in storage_sources['staging.rs']
assert 'validate_recovery_envelope(' in storage_sources['staging_files.rs']
assert 'pub fn stage_capture_v2(' in storage_sources['staging_files.rs']
assert 'pub fn reopen_staged_v2(' in storage_sources['staging_files.rs']
reopened_v2 = storage_sources['staging_files.rs'].split('pub struct ReopenedV2 {', 1)[1].split('}', 1)[0]
assert 'pub envelope:' not in reopened_v2 and 'pub pixels:' not in reopened_v2
assert 'pub fn pixels(&self) -> &[u8]' in storage_sources['staging_files.rs']
assert 'pub async fn stage_capture_owned_v2(' in storage_sources['lib.rs']
assert 'StageV2(' in storage_sources['lib.rs'] and 'stage_capture_v2(&root, v2)' in storage_sources['lib.rs']
assert 'warnings_to_json(&self.warnings)' in storage_sources['lib.rs']
assert 'pub async fn reopen_stage_owned_v2(' in storage_sources['lib.rs']
assert 'ReopenV2(' in storage_sources['lib.rs'] and 'staging_files::reopen_staged_v2(' in storage_sources['lib.rs']
assert 'staging::StagingExpectations {' in storage_sources['lib.rs']
assert 'pub async fn enumerate_stage_candidates(' in storage_sources['lib.rs']
assert 'EnumerateCandidates(oneshot::Sender<Result<StagingCandidates, EnumerateCandidatesError>>)' in storage_sources['lib.rs']
assert 'root.enumerate_candidates()' in storage_sources['lib.rs'].split('fn file_loop(', 1)[1]
assert 'root.enumerate_candidates()' not in storage_sources['lib.rs'].split('fn run(', 1)[1].split('fn file_loop(', 1)[0]
assert 'root.enumerate_candidates()' not in storage_sources['lib.rs'].split('fn start_inner(', 1)[1].split('pub async fn stage_capture_owned(', 1)[0]
assert 'pub envelope:' not in reopened_v2 and 'pub pixels:' not in reopened_v2
for forbidden in ('std::fs', 'std::path', 'rusqlite', 'std::process', 'std::net'):
    assert forbidden not in storage_sources['staging_recovery_tests.rs']
assert 'pub mod staging;' in storage_sources['lib.rs']
assert 'pub mod staging_files;' in storage_sources['lib.rs']
# S02-T09b manifest-last coordinator: it wraps the borrowed lease capability only. It may
# not own a path, a SQLite connection, a process, a wire or a host `durable_input` claim,
# and it must validate through the same pure validator before writing.
for forbidden in ('std::fs::', 'std::path', 'PathBuf', 'rusqlite', 'std::process', 'std::net',
                  'durable_input', 'Saved('):
    assert forbidden not in storage_sources['staging_files.rs'], ('storage', 'staging_files.rs', forbidden)
assert 'validate_staging(' in storage_sources['staging_files.rs']
assert 'StagingTempKind::ManifestJson' in storage_sources['staging_files.rs'], \
    'the manifest must be the last published artifact'
assert 'std::fs::' not in storage_sources['staging.rs'], ('storage', 'staging.rs')
assert {path.name for path in (root / 'crates/storage/migrations').glob('*.sql')} == {'0001_foundation.sql', '0002_images.sql', '0003_operations.sql'}
images_sql = (root / 'crates/storage/migrations/0002_images.sql').read_text(encoding='utf-8')
assert "format IN ('webp_lossless')" in images_sql and "'avif'" not in images_sql
assert "asset_type IN ('original', 'preview', 'edited')" in images_sql
assert 'PRIMARY KEY AUTOINCREMENT' in images_sql, 'bounded pages need the ordering key'
assert '"assets/{file_relative}"' in storage_sources['images.rs']
assert '"{month}/{record_id}/{asset_type}.webp"' in storage_sources['images.rs']
assert 'BEGIN IMMEDIATE' not in images_sql and 'TransactionBehavior::Immediate' in storage_sources['images.rs']
# S02-T09a journal replay: the saved row keeps the replayable receipt metadata, and the
# closed mode vocabulary and positive geometry bounds live in the migration itself.
operations_sql = (root / 'crates/storage/migrations/0003_operations.sql').read_text(encoding='utf-8')
assert "mode IN ('region', 'active_window', 'monitor', 'desktop')" in operations_sql, \
    'the journal mode vocabulary must match the typed CaptureTarget names, not the wire aliases'
assert 'width > 0 AND width <= 16383' in operations_sql and 'height > 0 AND height <= 16383' in operations_sql
assert "json_type(warnings) = 'array'" in operations_sql
application_storage = (root / 'crates/application/src/storage.rs').read_text(encoding='utf-8')
assert 'sm_domain::{AssetId, RecordId}' in application_storage
assert 'Page<Record>' in application_storage and 'RecordPageQuery' in application_storage
assert 'CANONICAL_FORMAT_WEBP_LOSSLESS' in application_storage
assert 'LookupOutcome' in application_storage and 'Saved(' in application_storage and 'Conflict' in application_storage
assert "'avif'" not in application_storage, 'the images vocabulary has no AVIF value'
codec_sources = {source.name: source.read_text(encoding='utf-8') for source in (root / 'crates/codec/src').rglob('*.rs')}
assert set(codec_sources) == {'lib.rs', 'error.rs', 'limits.rs', 'native.rs'}
assert {path.name for path in (root / 'crates/codec/tests').glob('*.rs')} == {'guards.rs', 'roundtrip.rs'}
for name, text in codec_sources.items():
    for forbidden in ('println!', 'eprintln!', 'log::', 'tracing::', 'std::fs::', 'std::env::'):
        assert forbidden not in text, ('codec', name, forbidden)
assert 'exact = 1;' in codec_sources['native.rs'], 'exact lossless is the encoded profile'
assert 'WEBP_EXACT_LOSSLESS_PROFILE' in codec_sources['lib.rs']
assert 'CodecCapability::NotImplemented' in codec_sources['error.rs'], 'AVIF is reported unimplemented'
assert 'system-dylib' not in codec_sources['lib.rs']
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
# S02 native capture: the owner keeps GDI/overlay state private and returns owned pixels.
capture_sources = {source.name: source.read_text(encoding='utf-8') for source in (root / 'crates/capture/src').rglob('*.rs')}
assert set(capture_sources) == {'lib.rs', 'geometry.rs', 'gdi.rs', 'window.rs', 'context.rs', 'overlay.rs', 'hotkeys.rs', 'host.rs'}
assert {path.name for path in (root / 'crates/capture/tests').glob('*.rs')} == {'live_capture.rs'}
for name, text in capture_sources.items():
    assert 'unsafe impl Send' not in text and 'unsafe impl Sync' not in text, ('capture', name)
    assert 'std::thread::spawn' not in text or name == 'host.rs', ('capture', name)
assert 'CreateDIBSection(' in capture_sources['gdi.rs'] and 'GetDIBits(' not in capture_sources['gdi.rs'], 'the only freeze path is a top-down DIB section'
assert 'CAPTUREBLT' in capture_sources['gdi.rs'] and 'NOMIRRORBITMAP' in capture_sources['gdi.rs']
assert 'GetMessageW' in capture_sources['overlay.rs'] and 'received == 0' in capture_sources['overlay.rs'] and 'received > 0' in capture_sources['overlay.rs'], 'GetMessageW has three decoded outcomes'
assert 'PostQuitMessage(' not in capture_sources['overlay.rs'], 'a finished selection never ends the thread'
assert 'signed_axis' in capture_sources['overlay.rs'], 'mouse coordinates are read as signed halves'
assert 'RegisterHotKey' in capture_sources['hotkeys.rs'] and 'MOD_NOREPEAT' in capture_sources['hotkeys.rs']
application_capture = (root / 'crates/application/src/capture.rs').read_text(encoding='utf-8')
assert 'pub trait NativeCaptureAdapter' in application_capture, 'the native port lives in the application seams'
assert 'windows_sys::' not in application_capture and 'unsafe' not in application_capture, 'the application seam stays platform free'
host_capture = (root / 'crates/host/src/capture.rs').read_text(encoding='utf-8')
assert 'commit_image_capture' in host_capture and 'encode_exact_lossless_verified' in host_capture, 'the host publishes only a verified canonical asset'
assert 'provenances' in host_capture and 'Provenance::Automatic' in host_capture
assert 'stage_capture_owned_v2(' in host_capture and 'staged.warnings()' in host_capture
assert 'stage_capture_owned(' not in host_capture and 'reopen_staged_v2(' not in host_capture
assert 'windows_sys::' not in host_capture and 'sm_capture::' not in host_capture.replace('sm_capture::HotkeySink', '').replace('sm_capture::HotkeyReservation', ''), 'the host never touches GDI directly'
protocol_capture = (root / 'crates/protocol/src/application.rs').read_text(encoding='utf-8')
assert '"capture.command"' in protocol_capture and '"library.asset.read"' in protocol_capture
assert 'struct CaptureCommandParams' in protocol_capture and 'enum CaptureOutcome' in protocol_capture
session_sources = {source.name: source.read_text(encoding='utf-8') for source in (root / 'crates/session/src').rglob('*.rs')}
assert '"capture.command"' in session_sources['lib.rs'] and 'Scope::CaptureScreen' in session_sources['lib.rs']
print('S01-DEP-01/S02-DEP-02 PASS: fifteen crates, exact dependency allowlists, bundled SQLite in storage, bundled static libwebp in the codec, closed images schema vocabulary and the diagnostic-window client boundary retained')
