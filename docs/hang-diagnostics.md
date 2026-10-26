# Local foreground hang diagnostics

The native macOS app enables GPUI's `profiler` feature and starts its existing
`App::start_hang_monitor` once, at the beginning of the application callback.
There is no settings change, network upload, extra heartbeat, or notification.
CLI-only invocations and `--check-data` do not start capture.

## Location and retention

Records live beside the **selected database**, in `diagnostics/`. The normal
installed-app path is:

```
~/Library/Application Support/com.vorkspace.desktop/diagnostics/hang-0.jsonl
```

The database resolver can retain a legacy application-data location; in that
case its sibling `diagnostics/` is used. `--vorkspace-db PATH` uses `PATH`'s
parent. `--dev-data` uses
`~/Library/Application Support/vorkspace Native Development/diagnostics/`, or
`$VORKSPACE_NATIVE_DATA_DIR/diagnostics/` when that existing override is set.
These are local files that survive restart. Capture starts lazily on the first
qualifying incident; an empty directory or absent files is normal.

`hang-0.jsonl` is current; `hang-1.jsonl` and `hang-2.jsonl` are older generations.
Each is capped at **1,048,576 logical bytes**, including newlines. Rotation
happens before an append would exceed that cap, deleting only the oldest
reserved generation. The three files together cannot exceed **3,145,728 logical
bytes** under this writer. A zero-byte `hang.lock` prevents concurrent app
instances from writing the same directory. Other filenames are untouched.
There is no age-based retention. Existing oversized reserved files disable
capture instead of being truncated. An incomplete final line from an interrupted
write is preserved in an archive before further appends; readers should skip
malformed lines. Rotation preserves recent evidence, not a transactional history:
a crash during renames can leave fewer generations.

The byte cap excludes filesystem block rounding, directory metadata and the lock
file's metadata. Each JSONL record is at most 32 KiB, with at most eight
contributors and 160 UTF-8 bytes per source/type string. At most eight incidents
are written per poll; `incidents_elided_in_poll` reports further qualifying
incidents discarded from that poll. No unbounded application buffering or
cross-poll backlog is introduced.

## Measurement and interpretation

The monitor drains GPUI's bounded foreground journal every **one second** on a
dedicated OS thread. A **500 ms** single-event threshold identifies actual
completed foreground stalls. GPUI's no-frame blocking-work budget is **32 ms**;
its display-aware late-frame rules still run internally. The local writer
retains only threshold-qualified incidents, so ordinary late frames, short
blocking work, and empty polls do not grow files. A long sequence of smaller
pieces of work can therefore miss this local filter even when it feels slow.

The explicit schema includes run start time, observation time, process ID, app
package version, measured longest stall, active interval duration, busy fraction,
frame delay, small-poll totals, journal loss/discontinuity counters and contributor
elision counts. Contributor fields contain only timing, nesting depth,
compile-time action/view type names, input **variant names**, source file/line/
column, and draw-phase timing. Build-machine prefixes are removed from source
locations. The schema does not serialize actions or other GPUI objects wholesale.
It never includes input text, prompts, transcripts, arbitrary command arguments,
provider payloads, credentials, window titles, or project/database paths.

`phase` is `startup` until the first observed newly drawn frame has been
submitted, then `steady`. Capture cannot cover database preparation or work
before GPUI's application callback. `during_active_use` means deliberate input
occurred in the preceding GPUI active-use window (60 seconds); it is **not** an
exact focus or visibility flag. Passive mouse movement does not establish active
use. `sealed_by` distinguishes presentation, skipped frame, idle and power
transition. For a skipped frame, `frame_skip_reason` distinguishes inactive-window
rate limiting, thermal rate limiting and no render needed. These are boundary
metadata, not proof of the stall's cause. A null skip reason or false active-use
flag cannot establish that a window was occluded.

GPUI excludes work spanning detected suspend/wake transitions. Its frame
measurement rules invalidate dirty-frame latency across hidden/occluded visibility
changes and power transitions; retained task/action/draw timings can still describe
real work while the app is in the background. There is no fabricated foreground
classification for those events. `measurement_version` identifies GPUI's rules.
A journal discontinuity weakens cumulative/boundary conclusions even when an
individual threshold event remains useful.

The longest contributor provides timing attribution, **not a stack trace**.
Nested durations overlap and must not be summed across depths. Events are
usually published when work completes, and an incident needs a sealing boundary.
A beachball that recovers may be recorded later; force quitting, a permanent
stall, or journal overwrite can leave no record. Restart does not reconstruct
missing events. Wall-clock observation time can change with system clock
adjustments; monotonic timings relative to this monitor's startup are used for
stall durations.

## Failure and lifecycle

All encoding, file opening, synchronization and rotation occur in GPUI's monitor
callback on its worker thread. Files are owner-only (`0600`) and the diagnostics
directory is private (`0700`). Descriptor-relative operations reject symlinks at
any path component, hard-linked/nonregular log files, other-owner or shared-mode
reserved files, and unsafe relative/parent paths. Existing reserved files are
never truncated in place. A symlinked application-data ancestor also disables
capture; this deliberately favors application continuity over following aliases.

An unavailable path, an existing unsafe file, a lock held by another instance,
write/rotation failure, or inability to start the monitor leaves the app running.
A storage failure disables writing for the rest of that run, without recurring
foreground retries or error messages containing private paths. A later app run
can try again. The monitor continues draining its bounded journal after a storage
failure. New records are synchronized after each append. GPUI requests a final
worker poll and flush during shutdown within its existing **200 ms total shutdown
budget**, shared with quit handlers; no unbounded main-thread join is added.
Filesystem operations can still be delayed by the OS, and shutdown/crash durability
is best effort.

Enabling the profiler adds timestamp collection and journal work on the foreground
thread, per-window statistics, a dedicated thread, and worker-side conversion and
I/O. GPUI budgets its journal ring at about 4 MiB plus slot/collector/interval and
statistics overhead; each interval caps events at 16,384. The application's
encoding buffer is capped at 32 KiB and records are written individually. Full
legacy tracing buffers are not enabled by this module. These limits do not prove
negligible CPU cost or total resident-memory usage; representative production
overhead has not been measured.

## Focused verification

Use a private checkout target to avoid sharing Cargo locks. The tests synthesize
GPUI journal events with explicit timestamps, run its interval sealer and hang
detector, and verify local JSONL timing/type attribution without sleeping through
a real hang or launching the installed app:

```
env -u CODEX_SQLITE_HOME CARGO_TARGET_DIR="$PWD/target/hang-diagnostics" CARGO_BUILD_JOBS=2 cargo clippy -p vorkspace-gpui --tests --profile native-dev -- -D warnings
env -u CODEX_SQLITE_HOME CARGO_TARGET_DIR="$PWD/target/hang-diagnostics" CARGO_BUILD_JOBS=2 cargo test -p vorkspace-gpui --profile native-dev hang_diagnostics:: -- --test-threads=1
env -u CODEX_SQLITE_HOME CARGO_TARGET_DIR="$PWD/target/hang-diagnostics" CARGO_BUILD_JOBS=2 cargo build -p vorkspace-gpui --profile native-dev
```

The focused tests cover threshold filtering, bounded encoding, source/input
allowlisting, startup/power/loss labels, temporary-directory persistence,
rotation/reopen byte caps, interrupted tails, empty flushes, private permissions,
unsafe files/links, concurrent writers, failure disablement, next-run recovery
and GPUI setup/shutdown. They establish the local data path and record contract,
not installed-app beachball reproduction or measured performance overhead.
