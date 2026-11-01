# Native visuals, in plain English

An agent can add an interactive chart, calculator or small 3D explanation to a
chat. The preview draws its controls directly. The saved conversation retains
the visual’s program; reopening it resets its interactive state.

The visual code has limited time and memory and cannot read files, use the
network or start programs. Its time limit counts only the time it spends
running, so a busy Mac cannot make a good visual fail. An invalid visual shows an error inside its own view.
Pi and omp receive a short command reference when needed; other supported agents use the available session tools.
The sample dashboard and 3D drawing programs live in
`crates/vorkspace-gpui/fixtures/visuals/`, alongside the app's other examples.
See [the visual API](../visuals.md) for examples and limits.

## Diagrams

Agents can also draw diagrams. When a reply, note or Bot document contains a Mermaid diagram (a simple text format for flowcharts, sequences and the like) or a picture written as SVG, the app draws it right there instead of showing the code. Mermaid diagrams are drawn by a small open-source library called Pele and take on the app's colours, so they look right in light and dark mode. Pictures can't run code or load anything from your computer or the internet. The app also lets go of drawings you've moved on from once they add up, and redraws one if you come back to it.

Agents also get a short set of drawing rules, borrowed from the diagram-design project: keep diagrams small, highlight only one or two things, use straight right-angle lines, and leave out anything that doesn't help.

