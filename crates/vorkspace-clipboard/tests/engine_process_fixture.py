#!/usr/bin/env python3
"""Explicit offline lifecycle check using helper processes and existing models.

Provide a debug helper, two verified models, and an isolated lock/log directory.
This opens no microphone, field, socket, or model download. It kills only the
fixture child used to simulate a crash. The models are read but never modified.
"""
import argparse
import queue
import subprocess
import threading
from pathlib import Path

parser = argparse.ArgumentParser()
parser.add_argument('--helper', type=Path, required=True)
parser.add_argument('--unified', type=Path, required=True)
parser.add_argument('--tdt', type=Path, required=True)
parser.add_argument('--directory', type=Path, required=True)
args = parser.parse_args()
assert all(p.is_absolute() for p in (args.helper, args.unified, args.tdt, args.directory))
args.directory.mkdir(parents=True, exist_ok=True)
lock = args.directory / 'fixture-engine.lock'
children = []

class Child:
    def __init__(self, name, fixture, model, kind):
        self.name = name
        self.events = queue.Queue()
        self.lines = []
        self.err = (args.directory / f'{name}.stderr').open('w')
        self.p = subprocess.Popen([str(args.helper), fixture, str(lock), str(model), kind],
                                  stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                                  stderr=self.err, text=True, bufsize=1)
        children.append(self)
        def read():
            for line in self.p.stdout:
                self.lines.append(line.rstrip())
                self.events.put(line.rstrip())
            self.events.put(None)
        self.reader = threading.Thread(target=read, daemon=True)
        self.reader.start()

    def expect(self, prefix, timeout=75):
        while True:
            line = self.events.get(timeout=timeout)
            assert line is not None, (self.name, prefix, self.lines, self.p.poll())
            if line.startswith(prefix):
                return line

    def command(self, value):
        self.p.stdin.write(value + '\n')
        self.p.stdin.flush()

    def finish(self):
        assert self.p.wait(timeout=20) == 0, (self.name, self.lines)

    def save(self):
        self.reader.join(timeout=5)
        (args.directory / f'{self.name}.log').write_text('\n'.join(self.lines) + '\n')
        self.err.close()

service = '--dictation-engine-service-fixture'
owner = '--dictation-engine-owner-fixture'
try:
    first = Child('service-unified', service, args.unified, 'unified')
    assert 'loads=1' in first.expect('READY ')
    second = Child('service-tdt', service, args.tdt, 'tdt-v3')
    waiting = second.expect('WAITING ')
    assert 'loads=0' in waiting and 'Disable dictation' in waiting and 'retrying automatically' in waiting
    first.command('stop')
    first.expect('JOINED '); first.finish()
    assert 'loads=1' in second.expect('READY ')
    blocked = Child('service-contender', owner, args.unified, 'unified')
    blocked.expect('BUSY'); blocked.finish()
    second.command('stop')
    second.expect('JOINED '); second.finish()

    first = Child('join-owner', owner, args.unified, 'unified')
    first.expect('READY')
    first.command('active'); first.expect('ACTIVE')
    blocked = Child('active-contender', owner, args.tdt, 'tdt-v3')
    blocked.expect('BUSY'); blocked.finish()
    first.command('cancel')
    first.expect('STOPPED_BEFORE_JOIN ok')
    blocked = Child('join-contender', owner, args.tdt, 'tdt-v3')
    blocked.expect('BUSY'); blocked.finish()
    first.command('join'); first.expect('JOINED'); first.finish()

    failed_service = Child('service-failed', service, args.directory / 'missing.gguf', 'unified')
    assert 'off and on to retry' in failed_service.expect('FAILED ')
    failed_service.command('stop'); failed_service.expect('JOINED '); failed_service.finish()
    failed = Child('failed-owner', owner, args.directory / 'missing.gguf', 'unified')
    failed.expect('STOPPED_BEFORE_JOIN error')
    blocked = Child('failed-contender', owner, args.tdt, 'tdt-v3')
    blocked.expect('BUSY'); blocked.finish()
    failed.command('join'); failed.expect('JOINED'); failed.finish()

    crash = Child('crash-owner', owner, args.unified, 'unified')
    crash.expect('READY')
    crash.p.kill()  # only this disposable fixture child
    crash.p.wait(timeout=20)
    recovered = Child('crash-recovery', service, args.tdt, 'tdt-v3')
    assert 'loads=1' in recovered.expect('READY ')
    recovered.command('stop'); recovered.expect('JOINED '); recovered.finish()
    print('PASS: production-service contention and automatic retry; active actual-worker cancellation, '
          'failed load, join barrier, and crash recovery across helper processes')
finally:
    for child in children:
        if child.p.poll() is None:
            child.p.kill()
            child.p.wait(timeout=20)
        child.save()
