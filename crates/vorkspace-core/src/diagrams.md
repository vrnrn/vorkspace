Diagrams
Draw a diagram only when it explains structure or flow better than a table or
a paragraph. The app renders three forms natively, in replies, Notes and Bot
documents:
- A fenced ```mermaid block, rendered by Pele (Mermaid 12 syntax: flowchart,
  sequence, state, class, ER, gantt, pie, xychart and the rest).
- A fenced ```svg block holding one <svg> element.
- In a visual: visual.mermaid(source,{title?}) or visual.svg(markup,{title?}).
HTML documents may also inline <svg> or use <pre class="mermaid">.
The app themes Mermaid diagrams itself: never add %%{init}%% themes, classDef
colours or style lines. SVG is static: no scripts, animation, foreignObject,
external images or fonts.

Design (adapted from the diagram-design skill by Cathryn Lavery, MIT):
- The highest-quality move is usually deletion. Every node is a distinct idea;
  merge nodes that always travel together. Remove any arrow whose relationship
  the layout already shows. Target density 4/10.
- Budget per diagram: at most 9 nodes, 12 arrows and 2 accent elements. Above
  that, split into an overview and a detail diagram.
- One accent: put it on the 1–2 focal elements only; everything else is ink,
  muted or a hairline. No glows, gradients, shadows or neon dark palettes.
  Borders, not shadows; radii 4–8, never pill-shaped boxes.
- Connectors are orthogonal: right-angle elbows (radius 6–8), never diagonals.
  Draw arrows before boxes. Give each connector on one edge its own attach
  point, 12px or more apart; never route a line behind a box it does not
  connect; never stack two strokes.
- Arrow labels: 14 characters or fewer, on an opaque background rect with a
  6–10px gap from the line. No vertical text.
- Lay out on a 4px grid. A legend, if needed, is a horizontal strip at the
  bottom covering exactly the types used.
- Names in the regular sans face at weight 600; monospace only for technical
  content such as ports, commands and paths.
- Hand-written SVG: a viewBox sized to the content (about 640–960 wide),
  role="img" and aria-labelledby naming a <title> (60 characters or fewer, the
  first child) and a <desc> (one sentence about the content), with prefixed
  IDs such as auth-flow-title. Use currentColor or neutral greys with one
  accent so it reads in light and dark themes.
- Before showing it, ask: can any node, arrow or label go? Is the accent on at
  most two elements? Would a table do the same job?
