#!/usr/bin/env python3
"""Check trailing words during a quiet noisy pause on each existing model."""
import argparse
import ast
import hashlib
import os
import random
import re
import struct
import subprocess
import wave
from pathlib import Path

parser = argparse.ArgumentParser()
for name in ('helper', 'wav', 'unified', 'tdt', 'flash', 'directory'):
    parser.add_argument('--' + name, type=Path, required=True)
args = parser.parse_args()
assert all(p.is_absolute() for p in vars(args).values())
args.directory.mkdir(parents=True, exist_ok=True)
assert hashlib.sha256(args.wav.read_bytes()).hexdigest() == '59dfb9a4acb36fe2a2affc14bacbee2920ff435cb13cc314a08c13f66ba7860e'
with wave.open(str(args.wav), 'rb') as wav:
    assert (wav.getframerate(), wav.getnchannels(), wav.getsampwidth()) == (16000, 1, 2)
    original = wav.readframes(wav.getnframes())
rng = random.Random(71)
noise = [rng.randint(-328, 328) for _ in range(4 * 16000)]
wav_path = args.directory / 'jfk-noisy-pause.wav'
with wave.open(str(wav_path), 'wb') as wav:
    wav.setnchannels(1)
    wav.setsampwidth(2)
    wav.setframerate(16000)
    wav.writeframes(original + struct.pack('<%dh' % len(noise), *noise))
env = dict(os.environ, VORKSPACE_DICTATION_FIXTURE_LOCK=str(args.directory / 'noise-engine.lock'))
for model, kind in ((args.unified, 'unified'), (args.tdt, 'tdt-v3'), (args.flash, 'eou')):
    result = subprocess.run([str(args.helper), '--live-dictation-fixture', str(model),
                             str(wav_path), kind, '1', '1', '0',
                             'Kennedy\nfellow Americans => Fellow Americans'],
                            env=env, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
                            text=True, timeout=120)
    (args.directory / (kind + '.log')).write_text(result.stdout)
    (args.directory / (kind + '.stderr')).write_text(result.stderr)
    assert result.returncode == 0, (kind, result.returncode, result.stderr[-2000:])
    summary = re.search(r'first_stable_ms=(\d+) recording_ms=(\d+).*final=(".*")', result.stdout)
    assert summary, (kind, result.stdout)
    stop = int(summary.group(2))
    updates = [(int(ms), ast.literal_eval(text)) for ms, text in re.findall(
        r'turn=1 committed_ms=(\d+) text=(".*")', result.stdout)]
    assert all(b.startswith(a) and b != a for (_, a), (_, b) in zip(updates, updates[1:]))
    final_country = [ms for ms, text in updates if ms < stop and text.lower().count('country') >= 2]
    assert final_country, (kind, stop, updates)
    print(f'PASS: {kind} last_word_ms={final_country[0]} stop_ms={stop} custom_words=true append_only=true', flush=True)
