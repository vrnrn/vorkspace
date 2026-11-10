# Native terminal

The project dock and terminal tabs use the same GPUI `Terminal` view. Each entity owns one login shell and one terminal state. Hiding a dock or switching tabs keeps the shell, screen, scrollback and selection alive. Only tab identity and the starting directory survive app restart; restored tabs start fresh shells when first selected.

## Emulator and renderer

`crates/vorkspace-terminal` wraps the public `ghostty/vt.h` interface of libghostty-vt. Ghostty parses the PTY byte stream and owns screen cells, terminal modes, reflow, scrollback, tracked selection endpoints and terminal replies. A small C adapter keeps upstream enums and sized structs at the C boundary. The safe Rust owner allows access only through mutable methods and can move between threads; GPUI and the PTY worker serialize access with a mutex.

GPUI captures Ghostty render state into owned rows, then releases the terminal lock before shaping or painting text. Cells retain complete UTF-8 grapheme clusters and their terminal widths. The surface paints text, ANSI and true-color backgrounds, text attributes, selection and cursor shape. Each capture updates default text and cursor colors from the current app theme through public Ghostty setters; OSC color overrides survive theme changes and their reset commands restore the current defaults. Synchronized output captures the exact frame at the start of a hold and keeps it until the program releases the hold or the one-second limit expires. Scrolling and selection release a hold immediately.

The build disables Kitty graphics, snapshots and search. The app does not render inline terminal images or expose Ghostty's full application renderer. It uses no internal `ghostty.h` surface API.

Scrollback has a 2,000-line target and an 8 MiB allocation target per terminal. These are Ghostty page-based estimates; page granularity can retain slightly more than either target.

## Shell lifecycle

`terminal/pty.rs` owns a nonblocking PTY and a dedicated I/O worker. The login shell starts in a separate Unix session with the PTY as its controlling terminal. It inherits the app's terminal search path and uses `TERM=xterm-256color`, `COLORTERM=truecolor` and `TERM_PROGRAM=vorkspace`. The requested directory falls back to the home directory if unavailable.

The worker serializes user input, resize ioctls and Ghostty's replies. A macOS kqueue watches PTY readiness, control messages and process exit, so idle hidden terminals have no periodic I/O wakeups. Only render-hold expiry and final-output draining carry deadlines. Input and replies share an 8 MiB queue budget, charging retained buffer capacity plus a per-chunk metadata allowance; input is rejected visibly when full, and resize and close remain independent of that budget. It reads and parses output even while the terminal is hidden. Repaint notifications coalesce into one pending wakeup; authoritative terminal state is already updated before that notification is sent. Foreground-process polling runs only for visible terminals, with an explicit fresh query before close. An owned master-descriptor reference keeps an in-flight query from observing a reused file descriptor.

Closing the entity signals a control path independent of queued input. The worker enumerates foreground and background job groups in the shell's private session, sends hangup, and revalidates membership before escalating to kill. The shell stays unreaped until cleanup finishes so its session identity cannot be reused; descendants that transferred to a separate session are excluded. The worker flushes discarded input and pending echo before reaping, preventing macOS exit cleanup from waiting for PTY echo. A normal exit drains final PTY output and cleans up attached jobs before publishing the exit event. A descendant that keeps the slave open cannot keep an exited login shell's terminal active indefinitely.

## Input and selection

Ghostty's key encoder reads the terminal's current application-cursor and Kitty keyboard modes for presses, repeats and releases. Command shortcuts remain with the app except Command-Left/Right and Command-Backspace, which retain the terminal's line-editing behavior. Option-Left/Right move by words; Option otherwise acts as Meta. Tab, Shift-Tab and Escape use the same encoder.

Command-C copies the selection, Command-V pastes, and Command-A selects the complete retained screen. Bracketed paste strips embedded end markers before wrapping the text. Single-click drag selects cells; double-click selects words, triple-click selects lines, and Shift-click extends the current anchor. Tracked anchors follow output, scrolling and resize/reflow. Dragging beyond the top or bottom extends through history on a timer until release, focus loss or hiding the terminal.

Terminal escape sequences can write plain text to the clipboard, capped at 1 MiB per request. Terminal-initiated clipboard reads are disabled: the public VT read callback is synchronous, and it must not wait for the GPUI thread while holding terminal state. User-initiated paste reads the clipboard on the GPUI thread.

## Building and checking

The crate checksums and unpacks `vendor/libghostty-vt/source.tar.gz` and `dependencies.tar.gz` into Cargo's build directory, then builds the pinned static library with Zig 0.17.x. Set `ZIG` to use an explicit executable. The Zig build uses an offline package directory and baseline CPU instructions. Neither a system Ghostty installation nor a runtime Ghostty library is required. The bundle includes the linked code's license notices. Source provenance and update instructions are in `vendor/libghostty-vt/README.md`.

Run `npm run test:rust` for the terminal engine and desktop checks. To focus on the terminal, use the same verification profile and checkout output.

```sh
cargo nextest run --locked --cargo-profile verify -p vorkspace-terminal
cargo nextest run --locked --cargo-profile verify -p vorkspace-gpui -E 'test(terminal::)'
npm run target:prune
```

The desktop tests exercise keyboard mapping, background continuity, selection scrolling and real PTY startup, resizing, final-output delivery and shutdown. The GPUI test runtime requires a macOS graphics session. PTY tests need permission to create a PTY and launch a shell.
