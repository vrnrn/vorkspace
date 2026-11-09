# Finding your way around the repository

On your Mac, vorkspace is two programs: the main app, and a small helper that keeps clipboard history running after you quit. `crates/vorkspace-gpui` draws the desktop and handles clicks and typing. `crates/vorkspace-core` runs agents and Bots, saves work, and manages files. Smaller folders beside them hold pieces with a single job: `vorkspace-ui` for colors, settings and animation timing, `vorkspace-runtime` for running a session, `vorkspace-editor` for understanding code in the file editor, `vorkspace-model` for plain data, `vorkspace-macos` for talking to the Mac, `vorkspace-meetings` for meeting transcripts and notes, and `vorkspace-helper` for recording audio and keeping clipboard history after you quit. All but the helper run together in one desktop app, and keeping them separate means changing one rebuilds less.

`host` is the program that runs on a remote computer. It has its own saved conversations and agent processes, which can continue when your desktop disconnects.

`crates/vorkspace-terminal` interprets terminal output using Ghostty's library. Its included source and build dependencies live in `vendor/libghostty-vt`; the packaged app needs no separate Ghostty installation.

`vendor` holds our own copy of GPUI, the toolkit that draws the app. We change it directly and no longer follow the original project. It also holds a small repaired copy of `block`, a library GPUI's Mac code relies on.

Code only depends on code below it, so a change low down cannot quietly depend on something built on top of it. A test checks this rule.

`embedded` contains the small JavaScript helpers that ship inside the desktop: editor language tools and voice audio. `scripts` contains developer tools for building, testing, measuring, and packaging the app. `crates/vorkspace-gpui/fixtures` contains example visuals, while `vendor` and `licenses` keep dependency code and its legal notices.

Tests sit near the code they check. Extra pretend clients used by host tests live under `host/test-support`. `docs` explains the app, and `docs/eli5` explains the same topics in simpler language. Temporary build output, personal data, and work logs are kept separate from the maintained source.
