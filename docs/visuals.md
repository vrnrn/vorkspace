# Native session visuals

An agent can publish an interactive visual into an ordinary conversation. Ask for a chart, comparison, calculator, diagram, or 3D explanation. The visual stays in the transcript with the rest of the reply and remains usable after the turn ends. Bots use the same capability when running a conversation.


The app renders visual components directly through GPUI. Programs describe content and interactions; the renderer supplies fonts, colors, control appearance, spacing defaults, and responsive sizing. There is no browser surface or HTML document in this path.

## Agent workflow

Active sessions offer `visual_docs`, `visual_check`, and `visual_show` through the session's MCP server where the provider supports injected tools. Tool descriptions are short capability summaries. The full component API, styling rules, examples, and limits are returned by `visual_docs` when the agent prepares a visual. Visual instructions are not added to ordinary MCP session messages or the MCP server's initialization instructions. No permanent provider configuration is changed.

1. Call `visual_docs` with `{}` to load the reference when creating a visual.
2. Write a JavaScript function body that returns a synchronous render function.
3. Call `visual_check` with `{ "source": "..." }` and fix any reported execution or component errors.
4. Call `visual_show` with `{ "title": "Short descriptive title", "source": "..." }` during the current turn. An optional `id` identifies a publication.
5. Add supporting prose without repeating what the visual already explains.

Pi and omp use the executable fallback. These providers get a compact, optional CLI reference once per provider conversation, including existing conversations on their first successful turn after the update. A saved receipt prevents repeating it; a new provider conversation, account, or executable receives a new reference. The SDK is loaded only through `visual docs` or `visual --help`. There is no general instruction to produce visuals on ordinary replies.

The check executes the program and validates its native component tree and representative controls. It does not produce a screenshot. Publishing validates again before saving the visual and acknowledging the request.

The equivalent commands work with a source file:

```sh
vorkspace-gpui visual docs
vorkspace-gpui visual check --input /path/to/explanation.js
vorkspace-gpui visual show --input /path/to/explanation.js --title "Explanation"
```

Reading the reference and checking also work outside a running app. Showing requires the capability inherited by the agent of a live session. CLI discovery supplies the executable path; it is also available to the agent through `VORKSPACE_VISUAL_EXECUTABLE`.

## Programs and native controls

```js
const state = visual.state({ quantity: 4, price: 12 });

return () => visual.column([
  visual.text("Cost explorer", { size: "title" }),
  visual.stat("Total", "$" + state.quantity * state.price),
  visual.slider("Quantity", state.quantity, 1, 20,
    value => { state.quantity = value; }),
  visual.button("Reset", () => { state.quantity = 4; }),
], { gap: 16 });
```

Handlers update local state and the app rerenders the component tree. The script interpreter persists while that view is retained. Reset recreates its initial state. `Math.random()` starts from a repeatable source-based seed, so generated example data is consistent when the program is checked or reopened. The header icons switch to the syntax-highlighted JavaScript source and reset the visual; source mode also offers a copy icon. Visuals grow to their full content height and scroll with the conversation. The source viewer scrolls long programs within its code pane.

| Component | Signature |
| --- | --- |
| Layout | `column(children, {gap})`, `row(children, {gap})`, `grid(children, {columns, gap})` |
| Text | `text(value, {size, color})`, `stat(label, value, {detail, color})` |
| Data | `table(columns, rows)`, `chart(type, series, {labels, height})` |
| Actions | `button(label, handler, {disabled})`, `toggle(label, value, handler)` |
| Values | `slider(label, value, min, max, handler, {step})`, `input(label, value, handler, {placeholder})` |
| Choices | `tabs(labels, selectedIndex, handler)`, `select(label, options, selectedIndex, handler)` |
| Drawing | `canvas(shapes, {width, height})` |
| Geometry | `scene(objects, {width, height, camera})` |
| Diagrams | `mermaid(source, {title})`, `svg(markup, {title})` |

All functions above belong to `visual`. Option objects and their fields are optional unless the component needs the value. Chart types are `bar`, `line`, and `area`. Each series has a `label`, numeric `values`, and an optional semantic `color`. All series have equal lengths; labels, when supplied, match that length.

Canvas drawing uses `rect`, `circle`, `line`, `polyline`, and `label`. Coordinates refer to the declared canvas width and height, and the host scales the drawing to fit the conversation.

## Styling

Use `ink`, `muted`, `faint`, `accent`, `positive`, `negative`, `warning`, and `series1` through `series6` for colors. Use `caption`, `small`, `body`, `subtitle`, `title`, or `hero` for text sizes. Unsupported style properties and arbitrary color strings fail validation.

Let the app draw buttons, inputs, switches, and choice controls. Use layout components rather than painting controls onto a canvas. Prefer gaps around 8–16, restrained headings, clear units, and chart or scene heights around 220–360. The outer visual inherits the conversation's appearance rather than introducing a page background.

## Diagrams

Diagrams render natively in three places: visuals (`visual.mermaid`, `visual.svg`), Markdown (replies, Notes and Bot documents) and HTML documents.

- **Mermaid** source is rendered to SVG by the vendored [Pele](https://pele.run) bundle (`assets/pele.global.js`, MIT) in a warm QuickJS context per worker thread, with links and remote images off and a per-call deadline. Pele themes its output with CSS variables, which the native SVG renderer does not resolve, so `diagrams.rs` replaces every `var(--…)` with the live palette (or Pele's fallback) and evaluates `color-mix()` before rasterizing.
- **SVG** must be one static `<svg>` element: scripts, `foreignObject` and `javascript:` are rejected. It is parsed through GPUI's untrusted path, which resolves `<image>` references only to inline `data:` URIs, nested SVG included, so a drawing never reads files or the network. Its root takes the palette's text colour unless it sets its own, so `currentColor` follows the theme.
- Both are rasterized at twice their logical size through a GPUI image asset on the background executor, keyed by kind, source and palette, so a theme change redraws them. Until a diagram is drawn, or if it cannot be, Markdown shows its source and a visual shows a short notice.
- GPUI keeps an asset until it is removed, so `diagrams.rs` bounds retained rasters at an estimated 64 MB. Past that, the least recently drawn diagrams nothing can be showing are released, raster and sprite. A view in a window's latest frame, drawn or replayed from a cached parent, may still be painting what it last drew, so that diagram stays until the view redraws without it or leaves every frame; what is on screen can therefore exceed the budget. A view that left the frame has no cached paint to replay, so a released diagram is rasterized again when it returns.
- In Markdown, a fenced ```mermaid (or `mmd`) or ```svg block, at any nesting, is drawn once it can no longer grow: it has its closing fence, or its list item or quote has ended. A streaming reply therefore does not render every partial diagram.
- HTML documents with a `mermaid` or `language-mermaid` class, in any quote style or order, also receive Pele inside their sandboxed iframe, themed by the document's variables; other documents do not carry it. Inline `<svg>` already works there.

`visual docs` ends with drawing guidance adapted from Cathryn Lavery's [diagram-design](https://github.com/cathrynlavery/diagram-design) skill (MIT): delete before adding, at most 9 nodes, 12 arrows and 2 accent elements, orthogonal connectors, masked short labels, a 4px grid, no shadows or glows, and an accessible SVG title and description. Bot context points at it for diagrams in replies and documents.

## 3D geometry

Scenes accept boxes, spheres, cylinders, cones, planes, custom triangle meshes, and 3D polylines. Each object has a position, rotation in radians, scale, and semantic color. Mesh indices are triples of vertex indices. Geometry is transformed, clipped at the camera's near plane, lit, projected, and drawn with native GPUI paths. Dragging orbits the scene; Command/Control plus scroll changes its camera distance. Ordinary scrolling stays with the transcript.

```js
return () => visual.scene([
  visual.object({ kind: "box" }, {
    rotation: [0.2, 0.5, 0], scale: [2, 1, 1], color: "accent",
  }),
  visual.object({
    kind: "mesh",
    vertices: [[-1, 0, 0], [1, 0, 0], [0, 2, 0]],
    indices: [[0, 1, 2]],
  }, { position: [2, 0, 0], color: "series2" }),
], { height: 320 });
```

This renderer supports bounded geometry rather than arbitrary GPU shaders, external model files, or texture assets. Controls can alter geometry and camera parameters through state.

## Lifetime and limits

Visual source is saved as a dedicated transcript message, including programs split across storage chunks. Its native row remains outside folded tool activity. A loaded transcript keeps at most eight visual interpreters, discarding the least recently painted views as needed. Reopening a discarded view or restarting the app recreates the program's initial state; control state is not written into the conversation.

Each interpreter has a 16 MiB memory budget and a 60 ms budget of its thread's CPU time per call, so a runaway script is stopped but a busy machine cannot fail a valid one. Source is limited to 64 KiB, trees to 512 components and 16 nesting levels. Tables, chart series, drawing commands, text, and mesh geometry have additional validated limits. Invalid or interrupted programs show an error in their own view and can be reset.

Programs have no filesystem, network, process, DOM, module loading, or timer bindings. Controls change only their visual's local state. Publication is scoped to the calling session and requires an active turn; it cannot target another conversation.

## Local examples

The [dashboard example](../crates/vorkspace-gpui/fixtures/visuals/visual-dashboard.js) exercises charts, tabs, stats, sliders, switches, and tables. The [geometry example](../crates/vorkspace-gpui/fixtures/visuals/visual-scene.js) exercises 3D primitives, orbiting, geometry controls, and 2D drawing. Both examples are checked against the real interpreter in tests.

To seed them into an isolated development conversation:

```sh
cargo run --manifest-path Cargo.toml --locked -p vorkspace-gpui --profile native-dev -- --dev-data --visual-fixture
```

The fixture cannot be written into the production database. `cargo run` builds only the app, so to include clipboard history, run `cargo build --locked -p vorkspace-clipboard --profile native-dev` first. Without it, Settings reports that the clipboard helper did not start.
