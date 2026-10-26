# Local hang diagnostics, in plain English

Vorkspace's native Mac app keeps a small local notebook of completed foreground
work that took at least half a second. It uses the timing recorder already in
GPUI. There is no upload, popup or settings change.

For a normal installation, look here after restarting:

```
~/Library/Application Support/com.vorkspace.desktop/diagnostics/
```

The folder follows the database the app actually uses. Older installations can
use an older data folder. A custom database uses its own neighboring
`diagnostics` folder; development data uses its separate development folder.
Nothing is created until a qualifying delay is observed.

`hang-0.jsonl` contains the latest records. `hang-1.jsonl` and `hang-2.jsonl`
contain older ones. Each can hold at most 1 MiB, so the notebook holds at most
3 MiB of file contents. Filesystem bookkeeping uses additional space. Only these
reserved files rotate; other files are left alone. Old records disappear as new
ones fill the notebook. An interrupted final line may remain in an older file.

A record can say “this foreground task took ten seconds,” give its source
location, or identify an action/view type and slow drawing phase. It records
numbers and code names, not what you typed, conversations, prompts, commands,
credentials or provider responses. It does not contain a complete stack trace.

Startup records are labeled separately. Sleep/wake interruptions are excluded
by GPUI's timing rules. The notebook also keeps available activity and skipped-
frame clues, such as background-window rate limiting. Those clues cannot always
tell whether the app was in front or hidden. A slow task can run in the
background too. Nested timings overlap, so adding every duration together gives
a misleading result.

The recorder checks once a second and ignores ordinary short frame delays.
Work normally becomes visible to it only after that work finishes and GPUI
closes the timing interval. If a beachball recovers, a useful record may follow.
If the app never recovers or is force quit first, there may be no record. The
recorder also misses startup work before GPUI starts and delays made from many
smaller tasks that never individually reach half a second.

The app writes files on the recorder's own thread. It keeps at most eight causes
per record, limits text lengths and record size, and writes at most eight records
per check. Records say when information was dropped. The underlying recorder
also has memory limits. Timing collection costs CPU and memory; its real-world
overhead has not yet been measured.

The files are private to your user account. If the destination is unwritable,
linked somewhere else, unexpectedly unsafe, or already in use by another app
instance, capture stops for that run and the app keeps working. It can try again
on the next launch. It never repeatedly asks the foreground thread to fix files.
Normal shutdown requests one last background flush under GPUI's existing short
shutdown deadline. A crash or slow disk can still lose the last evidence.

The focused checks create artificial timing events and temporary files. They
verify the recorded numbers, privacy limits, file-size limits and failure
behavior without deliberately freezing or restarting your installed app. A
successful test does not prove that every real beachball will leave a record.
