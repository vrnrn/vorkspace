# Finding your way around the repository

`crates/vorkspace-gpui` draws the desktop and handles clicks and typing. `crates/vorkspace-core` runs agents, saves work, and manages files. They run together in one desktop app.

`host` is the program that runs on a remote computer. It has its own saved conversations and agent processes, which can continue when your desktop disconnects.

`embedded` contains the small JavaScript helpers that ship inside the desktop: editor language tools and voice audio. `scripts` contains developer tools for building, testing, measuring, and packaging the app. `examples` contains example visuals, while `vendor` and `licenses` keep dependency code and its legal notices.

Tests sit near the code they check. Extra pretend clients used by host tests live under `host/test-support`. `docs` explains the app, and `docs/eli5` explains the same topics in simpler language. Temporary build output, personal data, and work logs are kept separate from the maintained source.
