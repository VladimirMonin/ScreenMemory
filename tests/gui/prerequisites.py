"""Explicit installed native-window prerequisites; no installs or browser launch."""
import importlib.metadata
import json
import pathlib
import platform
import sys

PYTHON = pathlib.Path('C:/Python314/python.exe')
VERSIONS = {'playwright': '1.58.0', 'jsonschema': '4.26.0'}

def validate(version, machine, packages):
    if tuple(version) != (3, 14, 2) or machine != 'AMD64':
        raise RuntimeError('S06_PREREQUISITE_PYTHON_VERSION')
    if packages != VERSIONS:
        raise RuntimeError('S06_PREREQUISITE_PACKAGE_VERSION')

def check():
    if pathlib.Path(sys.executable).resolve(strict=True) != PYTHON.resolve(strict=True):
        raise RuntimeError('S06_PREREQUISITE_EXPLICIT_PYTHON')
    versions={name: importlib.metadata.version(name) for name in VERSIONS}
    validate(sys.version_info[:3], platform.machine(), versions)
    repo=pathlib.Path(__file__).resolve().parents[2]
    for name in ['inspect_search.py','capture_search.ps1','check_endpoint.ps1','close_search.ps1']:
        path=repo/'tests/gui'/name
        if not path.is_file() or getattr(path.lstat(),'st_file_attributes',0)&0x400:
            raise RuntimeError('S06_PREREQUISITE_TRACKED_HELPER')
    return dict(python='3.14.2 AMD64',packages=versions,browser_launch=False,downloads=False)

if __name__=='__main__':
    print(json.dumps(check(),sort_keys=True))
