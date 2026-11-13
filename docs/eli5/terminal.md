# Native terminal, simply

Each terminal tab or dock has its own shell. Switching away hides it while commands keep running and their output stays available. Closing it ends its shell and attached foreground or background jobs, including jobs that ignore the initial hangup. Programs that deliberately started a separate session remain independent. Reopening the app creates a fresh shell in the saved starting folder.

Ghostty interprets the shell's output and remembers the screen and previous lines. Vorkspace draws that screen with its own native interface and updates ordinary text and cursor colors when you change the theme. An idle hidden terminal waits for work instead of waking on a timer. It shows text, colors, selection and the cursor; inline terminal images are not supported.

The usual terminal keys work, including full-screen command-line programs. Command-C copies selected text, Command-V pastes, and Command-A selects all retained output. Option-Left and Option-Right move by words. Double-click selects a word; triple-click selects a line. Hold a selection drag beyond the top or bottom to keep selecting through older output.

A command can send text to the clipboard. It cannot read your clipboard automatically; pasting is a deliberate action you perform in the terminal.

Building the app also builds the included Ghostty library, so developers need Zig 0.17.x as well as the other build tools. Its source and dependencies are already included and the build works offline. People running the packaged app do not need to install Ghostty or Zig.

The regular Rust checks cover the terminal. [The technical guide shows how to run just those checks.](../terminal.md#building-and-checking)
