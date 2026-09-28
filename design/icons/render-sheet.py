"""Render with headless Chrome, then terminate it once the PNG is complete."""
import argparse
import os
from pathlib import Path
import signal
import struct
import subprocess
import tempfile
import time

HERE = Path(__file__).resolve().parent
CHROME = '/Applications/Google Chrome.app/Contents/MacOS/Google Chrome'
parser = argparse.ArgumentParser()
parser.add_argument('--output', default='icons-sheet.png')
args = parser.parse_args()
target = HERE / args.output
if target.parent != HERE or target.suffix != '.png':
    parser.error('Output must be a PNG filename directly under design/icons/.')
with tempfile.TemporaryDirectory(prefix='.chrome-profile-', dir=HERE) as profile:
    # Preserve any existing deliverable when Chrome fails before producing a PNG.
    pending = Path(profile) / 'capture.png'
    with (HERE / 'chrome-render.log').open('w') as log:
        command = [CHROME, '--headless=new', '--disable-gpu', '--no-first-run',
                   '--no-default-browser-check', '--disable-background-networking',
                   '--disable-breakpad', '--disable-crash-reporter',
                   f'--user-data-dir={profile}', f'--screenshot={pending}',
                   '--window-size=1600,1000', '--force-device-scale-factor=2',
                   '--hide-scrollbars', '--virtual-time-budget=2500',
                   (HERE / 'index.html').as_uri()]
        process = subprocess.Popen(command, stdout=log, stderr=log, start_new_session=True)
        complete = False
        try:
            deadline = time.monotonic() + 40
            while time.monotonic() < deadline:
                if pending.exists() and pending.stat().st_size > 32:
                    raw = pending.read_bytes()
                    # IEND proves Chrome has completed the PNG write.
                    if raw[:8] == b'\x89PNG\r\n\x1a\n' and raw[-12:-8] == b'\x00\x00\x00\x00' and raw[-8:-4] == b'IEND':
                        dimensions = struct.unpack('>II', raw[16:24])
                        if dimensions != (3200, 2000):
                            raise RuntimeError(f'Unexpected screenshot dimensions: {dimensions}')
                        complete = True
                        print(f'{target.name}: {dimensions[0]}×{dimensions[1]}, {len(raw):,} bytes')
                        break
                if process.poll() is not None:
                    break
                time.sleep(.2)
        finally:
            # Kill only this dedicated headless process group, never user's Chrome.
            if process.poll() is None:
                os.killpg(process.pid, signal.SIGTERM)
                try:
                    process.wait(timeout=3)
                except subprocess.TimeoutExpired:
                    os.killpg(process.pid, signal.SIGKILL)
                    process.wait(timeout=3)
        if not complete:
            raise SystemExit(f'Chrome did not produce a complete screenshot (exit {process.returncode}). See design/icons/chrome-render.log.')
        pending.replace(target)
