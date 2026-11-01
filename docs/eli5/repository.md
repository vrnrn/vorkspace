# Finding your way around the repository

`crates/vorkspace-gpui` draws the desktop and handles clicks and typing. `crates/vorkspace-core` runs agents, saves work, and manages files. Five smaller folders beside them hold pieces with a single job: `vorkspace-ui` for colors, settings and animation timing, `vorkspace-runtime` for running a session, `vorkspace-editor` for understanding code in the file editor, `vorkspace-model` for plain data, and `vorkspace-macos` for talking to the Mac. They all run together in one desktop app, and keeping them separate means changing one rebuilds less.

`host` is the program that runs on a remote computer. It has its own saved conversations and agent processes, which can continue when your desktop disconnects.

`vendor` holds our own copy of GPUI, the toolkit that draws the app. We change it directly and no longer follow the original project.

`embedded` contains the small JavaScript helpers that ship inside the desktop: editor language tools and voice audio. `scripts` contains developer tools for building, testing, measuring, and packaging the app. `crates/vorkspace-gpui/fixtures` contains example visuals, while `vendor` and `licenses` keep dependency code and its legal notices.

Tests sit near the code they check. Extra pretend clients used by host tests live under `host/test-support`. `docs` explains the app, and `docs/eli5` explains the same topics in simpler language. Temporary build output, personal data, and work logs are kept separate from the maintained source.
