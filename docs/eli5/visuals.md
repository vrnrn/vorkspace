# Native visuals, in plain English

An agent can add an interactive chart, calculator or small 3D explanation to a
chat. The preview draws its controls directly. The saved conversation retains
the visual’s program; reopening it resets its interactive state.

The visual code has limited time and memory and cannot read files, use the
network or start programs. An invalid visual shows an error inside its own view.
Pi and omp receive a short command reference when needed; other supported agents use the available session tools.
The sample dashboard and 3D drawing programs live in
`crates/vorkspace-gpui/fixtures/visuals/`, alongside the app's other examples.
See [the visual API](../visuals.md) for examples and limits.
