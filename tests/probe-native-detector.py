"""Optional real Linux detector probe. No Discord account, IPC, or network access.

Uses an already-installed official addon in an isolated child, not inside Discord.
Example: python3 tests/probe-native-detector.py --module /path/discord_utils.node
    --catalog /path/games-v1.json --helper /path/out/dummy --output result.json
Never copies or redistributes Discord's module. Not part of CI or app runtime.
"""
import argparse
import ctypes.util
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import time

parser = argparse.ArgumentParser(description=__doc__)
for name in ['module', 'catalog', 'helper']:
    parser.add_argument(f'--{name}', type=Path, required=True)
parser.add_argument('--output', type=Path)
args = parser.parse_args()
for f in [args.module, args.catalog, args.helper]:
    if not f.is_file():
        parser.error(f'Missing input file: {f}')
node = shutil.which('node')
if not node:
    parser.error('Node.js is required')
env = {k: os.environ[k] for k in ['DISPLAY', 'XAUTHORITY', 'WAYLAND_DISPLAY',
       'XDG_RUNTIME_DIR', 'DBUS_SESSION_BUS_ADDRESS', 'XDG_SESSION_TYPE'] if k in os.environ}
# Electron normally supplies XInput symbols. Standalone Node does not.
xi = ctypes.util.find_library('Xi')
if xi:
    env['LD_PRELOAD'] = xi
worker = Path(__file__).with_name('native-detector-worker.cjs')
with tempfile.TemporaryDirectory(prefix='mimic-native-probe-') as tmp:
    os.chmod(tmp, 0o700)
    result_path = Path(tmp) / 'result.json'
    process = subprocess.Popen([node, str(worker), str(args.module.resolve()),
        str(args.catalog.resolve()), str(args.helper.resolve()), tmp, str(result_path)],
        cwd=tmp, env=env, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    try:
        deadline = time.monotonic() + 20
        while not result_path.exists() and time.monotonic() < deadline:
            if process.poll() is not None:
                raise SystemExit(f'Native diagnostic worker exited ({process.returncode}); no result')
            time.sleep(0.05)
        if not result_path.exists():
            raise SystemExit('No probe result within 20 seconds')
        result = json.loads(result_path.read_text())
    finally:
        # Only our own child. Its helpers additionally have PR_SET_PDEATHSIG.
        if process.poll() is None:
            process.kill()
        process.wait(timeout=3)
for label, file in [('nativeModuleSha256', args.module), ('catalogSha256', args.catalog),
                    ('helperSha256', args.helper)]:
    with file.open('rb') as stream:
        result[label] = hashlib.file_digest(stream, 'sha256').hexdigest()
print(json.dumps(result, ensure_ascii=False, indent=2))
if args.output:
    args.output.write_text(json.dumps(result, ensure_ascii=False, indent=2) + '\n')
if not all(sample['matched'] for sample in result['samples']):
    raise SystemExit(1)
