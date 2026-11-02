#!/usr/bin/env python3
"""Check public JFK streaming with and without custom words, without a mic.

Supply the pinned public WAV and an existing Unified model. Long quiet gaps in
the public recording are shortened in a temporary copy, so the same speech can
be fed continuously and cannot accidentally satisfy a pause-only implementation.
"""
import argparse
import ast
import hashlib
import math
import os
import re
import struct
import subprocess
import wave
from pathlib import Path

parser = argparse.ArgumentParser()
parser.add_argument('--helper', type=Path, required=True)
parser.add_argument('--wav', type=Path, required=True)
parser.add_argument('--model', type=Path, required=True)
parser.add_argument('--directory', type=Path, required=True)
args = parser.parse_args()
assert all(p.is_absolute() for p in (args.helper, args.wav, args.model, args.directory))
args.directory.mkdir(parents=True, exist_ok=True)
assert hashlib.sha256(args.wav.read_bytes()).hexdigest() == '59dfb9a4acb36fe2a2affc14bacbee2920ff435cb13cc314a08c13f66ba7860e'
with wave.open(str(args.wav), 'rb') as wav:
    assert (wav.getframerate(), wav.getnchannels(), wav.getsampwidth()) == (16000, 1, 2)
    samples = struct.unpack('<%dh' % wav.getnframes(), wav.readframes(wav.getnframes()))
blocks = [samples[i:i + 1600] for i in range(0, len(samples), 1600)]

def quiet(block):
    return math.sqrt(sum(x*x for x in block) / len(block)) / 32768 <= .02

continuous = []
removed = 0
i = 0
while i < len(blocks):
    if not quiet(blocks[i]):
        continuous.append(blocks[i])
        i += 1
        continue
    j = i
    while j < len(blocks) and quiet(blocks[j]):
        j += 1
    run = blocks[i:j]
    continuous.extend(run[:2] if len(run) >= 9 else run)
    removed += max(0, len(run) - 2) if len(run) >= 9 else 0
    i = j
longest = current = 0
for block in continuous * 2:
    current = current + 1 if quiet(block) else 0
    longest = max(longest, current)
assert removed > 0 and longest < 10, (removed, longest)
wav_path = args.directory / 'jfk-continuous.wav'
with wave.open(str(wav_path), 'wb') as wav:
    wav.setnchannels(1)
    wav.setsampwidth(2)
    wav.setframerate(16000)
    for block in continuous:
        wav.writeframesraw(struct.pack('<%dh' % len(block), *block))

env = dict(os.environ, VORKSPACE_DICTATION_FIXTURE_LOCK=str(args.directory / 'public-engine.lock'))
def run(name, rules):
    result = subprocess.run([str(args.helper), '--live-dictation-fixture', str(args.model),
                             str(wav_path), 'unified', '2', '1', '0', rules], env=env,
                            stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True, timeout=120)
    (args.directory / (name + '.log')).write_text(result.stdout)
    (args.directory / (name + '.stderr')).write_text(result.stderr)
    assert result.returncode == 0, (name, result.returncode, result.stderr[-2000:])
    updates = [(int(ms), ast.literal_eval(text)) for ms, text in re.findall(
        r'turn=1 committed_ms=(\d+) text=(".*")', result.stdout)]
    summary = re.search(r'first_stable_ms=(\d+) recording_ms=(\d+).*final=(".*")', result.stdout)
    assert summary and updates, (name, result.stdout)
    recording = int(summary.group(2))
    final = ast.literal_eval(summary.group(3))
    assert all(b.startswith(a) and b != a for (_, a), (_, b) in zip(updates, updates[1:]))
    previous = ''
    meaningful = 0
    for ms, text in updates:
        if ms < recording and any(c.isalnum() for c in text[len(previous):]):
            meaningful += 1
        previous = text
    assert meaningful >= 3, (name, meaningful, updates)
    return updates, int(summary.group(1)), recording, final, meaningful

baseline, first_baseline, duration, baseline_final, _ = run('baseline', '')
custom, first_custom, custom_duration, final, meaningful = run(
    'custom', 'Kennedy\nfellow Americans => Fellow-Americans')
assert first_custom <= first_baseline + 800, (first_baseline, first_custom)
assert any('fellow' in text and 'fellow Americans' not in text for _, text in baseline)
assert any('fellow Americans' in text for _, text in baseline)
assert 'Fellow-Americans' in final and 'fellow Americans' not in final
assert any('Fellow-Americans' in text for ms, text in custom if ms < custom_duration)
assert baseline_final.count('fellow Americans') == 2 and baseline_final.lower().count('country') == 4
assert final == baseline_final.replace('fellow Americans', 'Fellow-Americans')
print(f'PASS: continuous public JFK; shortened_quiet_ms={removed*100}, '
      f'longest_quiet_ms={longest*100}, recording_ms={duration}, '
      f'baseline_first_ms={first_baseline}, custom_first_ms={first_custom}, '
      f'meaningful_live_updates={meaningful}, cross_chunk_alias=true, append_only=true')
