"""Capture HTML with a dedicated headless Chrome; stop after a complete PNG."""
import argparse
import os
from pathlib import Path
import signal
import struct
import subprocess
import tempfile
import time

HERE = Path(__file__).resolve().parent
parser = argparse.ArgumentParser()
parser.add_argument('--output', default='icons-round2-sheet.png')
args = parser.parse_args()
target = (HERE / args.output).resolve()
if not target.is_relative_to(HERE) or target.suffix != '.png':
    parser.error('PNG output must be within round2/.')
target.parent.mkdir(parents=True, exist_ok=True)
with tempfile.TemporaryDirectory(prefix='.chrome-profile-', dir=HERE) as profile:
    pending = Path(profile) / 'capture.png'
    command = ['/Applications/Google Chrome.app/Contents/MacOS/Google Chrome',
               '--headless=new', '--disable-gpu', '--no-first-run',
               '--no-default-browser-check', '--disable-background-networking',
               '--disable-breakpad', '--disable-crash-reporter',
               f'--user-data-dir={profile}', f'--screenshot={pending}',
               '--window-size=1600,1000', '--force-device-scale-factor=2',
               '--hide-scrollbars', '--virtual-time-budget=1500',
               (HERE / 'index.html').as_uri()]
    with (HERE / 'chrome-render.log').open('w') as log:
        process = subprocess.Popen(command, stdout=log, stderr=log, start_new_session=True)
        complete = False
        try:
            deadline = time.monotonic() + 35
            while time.monotonic() < deadline:
                if pending.exists() and pending.stat().st_size > 32:
                    raw = pending.read_bytes()
                    if raw.startswith(b'\x89PNG\r\n\x1a\n') and raw[-8:-4] == b'IEND':
                        dimensions = struct.unpack('>II', raw[16:24])
                        if dimensions != (3200, 2000):
                            raise RuntimeError(f'Unexpected dimensions: {dimensions}')
                        complete = True
                        break
                if process.poll() is not None:
                    break
                time.sleep(.2)
        finally:
            if process.poll() is None:
                os.killpg(process.pid, signal.SIGTERM)
                try:
                    process.wait(timeout=3)
                except subprocess.TimeoutExpired:
                    os.killpg(process.pid, signal.SIGKILL)
                    process.wait(timeout=3)
        if not complete:
            raise SystemExit(f'Chrome produced no complete PNG (exit {process.returncode}); see chrome-render.log.')
        pending.replace(target)
        print(f'Chrome capture: {target.relative_to(HERE)}, 3200 × 2000')
