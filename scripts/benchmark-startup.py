#!/usr/bin/env python3
"""Fresh-process launch timings. Does not measure compositor presentation or Dock bounce."""
import argparse
import json
import os
from pathlib import Path
import statistics
import subprocess
import tempfile
import time

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('--runs', type=int, default=7)
parser.add_argument('--stress', type=int, default=0)
args = parser.parse_args()
root = Path(__file__).resolve().parents[1]
app = root / 'build/Crawler.app'
results = []
with tempfile.TemporaryDirectory(prefix='crawler-startup-') as temp:
    for index in range(args.runs):
        report = Path(temp) / f'{index}.json'
        start = time.time()
        subprocess.run(['open', '-n', '-a', str(app), '--args', '--launch-report', str(report),
                        '--quit-after', '0.6', '--stress', str(args.stress)], check=True)
        deadline = time.monotonic() + 15
        while not report.exists():
            if time.monotonic() > deadline:
                raise RuntimeError('Launch report timed out')
            time.sleep(0.005)
        row = json.loads(report.read_text())
        row['open_to_ready_ms'] = (row['ready_epoch_seconds'] - start) * 1000
        results.append(row)
        print(f"{index + 1}: open → app-ready {row['open_to_ready_ms']:.1f} ms; app entry → display submission {row['entry_to_display_submit_ms']:.1f} ms", flush=True)
        while True:
            try:
                os.kill(row['pid'], 0)
            except ProcessLookupError:
                break
            if time.monotonic() > deadline:
                raise RuntimeError('Benchmark instance did not exit')
            time.sleep(0.02)
summary = {
    'method': 'Fresh processes via LaunchServices; warm OS caches; no cache purge. App-ready follows display submission and next main runloop, not compositor presentation. Literal Dock bounce requires visual review.',
    'stress_items': args.stress,
    'median_open_to_ready_ms': statistics.median(r['open_to_ready_ms'] for r in results),
    'max_open_to_ready_ms': max(r['open_to_ready_ms'] for r in results),
    'median_entry_to_display_submit_ms': statistics.median(r['entry_to_display_submit_ms'] for r in results),
    'runs': results,
}
output = root / 'artifacts' / f'startup-{args.stress}.json'
output.parent.mkdir(exist_ok=True)
output.write_text(json.dumps(summary, indent=2) + '\n')
print(f"Median {summary['median_open_to_ready_ms']:.1f} ms; maximum {summary['max_open_to_ready_ms']:.1f} ms\n{output}")
