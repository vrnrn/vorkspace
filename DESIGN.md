# Vorkspace Design Guide

Vorkspace should feel like one quiet, native macOS tool. Chrome recedes; the
conversation, code and user's work carry the color. Use this guide for every UI
change, and extend it when you add a new shared convention.

Paths are relative to `crates/vorkspace-gpui/src/`, except `theme.rs`, `radius.rs`,
`reveal.rs` and `pane_motion.rs`, which live in `crates/vorkspace-ui/src/`.

---

## 1. Principles

1. **Ink, not paint.** Neutral UI is drawn with the theme's content ink at an
   alpha (`theme::content(a)`), never with opaque greys or hex literals. Ink
   follows every workspace theme, light and dark, automatically.
2. **One owner per look.** A control, surface or token has one helper. Reuse it;
   if it doesn't fit, extend it at the owner instead of copying its styling into
   your file. Copies drift.
3. **Color means something.** Accent marks focus, selection and the primary
   action. Status hues mark state. Anything decorative stays neutral.
4. **Native first.** Vorkspace is macOS-only. Match macOS behavior: system font, arrow cursor on window
   chrome, instant dismissal, glass only where the platform provides it.
5. **Calm motion.** Animation explains a change of place or state and is
   short. There is no reduced-motion mode; animations always run.
6. **Every intermediate frame counts.** No flashes, jumps, re-played entrances
   or layout collapse during a transition (see AGENTS.md).

---

## 2. Color

### Print (`fluid.rs`)

Vorkspace borrows fluidither's printed look for surfaces that carry meaning:
tracks, gauges, charts, empty pages and notice strips. Hover, selection,
scrims, switches and diagram nodes stay flat; the pointer passing over a row
never prints a pattern, and the 50% checkerboard is never used. `vorkspace_ui::fluid` is the only owner; call
its helpers, never `gpui::dither` directly.

| Helper | Use |
|---|---|
| `hover(weight)`, `wash(tint)`, `pressed(weight)` | Flat hover and press tints |
| `track(weight)` | The empty part of a slider, meter or progress bar: a dotted rule (25% Bayer). The filled part is solid ink |
| `ring_track(color)` | A gauge's unfilled ring, dotted in 1pt cells |
| `area(color)` / `bar(color)` | Chart fills: solid under the line fading to nothing; bars full at the top printing down to three-quarter tone, never the half-tone checkerboard |
| `plate()` | The dotted plate behind an empty state's icon, fading from quarter tone to paper |
| `field(id)` | A still fluidither aurora field above the empty session prompt |
| `hint(weight)`, `hint_in(color, weight)` | Dotted washes for drop targets and split landing zones |
| `scrim(weight)` | A flat black dim behind dialogs; a printed scrim garbles the text it covers |
| `rule()` | Markdown's `---`: a line of dots |
| `strip(tone, radius)` | A notice's leading edge, as fluidither's Banner: a 3pt tone bar over a dotted column, one quad that follows the card's corners (error and approval toasts) |
| `badge(label, tone)` | A status label, as fluidither's Badge: a square tone cell, label at full contrast (inbox status) |
| `kbd(keys)` | A shortcut chip in Obel Mono on a soft pill (menu shortcuts); chips with macOS key symbols use the system font, which has them |
| `step_mark(step, color, slot)` | A step's mark, as fluidither's Steps: solid square done, outlined with dots active, empty outline waiting, a dash skipped (task lists) |
| `thumb(color)` | A slider's upright rounded-bar thumb, 8 by 16 |
| `knob(on)`, `switch_off()` | A switch: a white 3pt-rounded square knob (black on a pale accent) on a solid accent track when on, a flat `content(0.16)` track when off |
| `on_ink()` | Text and icons on a solid ink fill: primary buttons and the neutral composer submit |
| `is_numeric(text)` | Whether a table cell is a number; numeric cells use Obel Mono's tabular digits, and a visual's all-numeric columns right-align |
| `Count` | A whole number that rolls to each new value on the `QUICK` spring (the diff stat) |
| `Scramble` | Text that settles from stand-in glyphs over 0.7 s when it changes after its first showing (session titles in the sidebar) |

fluidither's own components draw with its theme and, in places, the 50%
checker, so vorkspace adopts their patterns through these helpers in its own
ink rather than mounting them. Working states use the loader cells
(`panel_kit::spinner`): busy controls, the live work receipt and Bot launch
cards.

The cells are drawn by the GPUI fork's `Background::Dither` (and its vertical
tone ramp, `vertical_dither`): one quad or path, clipped by its rounded corners,
with fluidither's 4×4 Bayer ranks, so a tone inks the same cells as fluidither
on the web. Cell sizes are logical points and scale to device pixels at paint.
Patterns use content ink or the series' own color; the user's accent stays the
only chrome color. Mermaid node surfaces stay flat, since a texture behind a
label costs it legibility. Chart value axes land on round steps (1, 2, 2.5 or
5 × 10ⁿ) that clear the tallest value (`visuals::nice_scale`).

### Tokens (`theme.rs`)

`theme::configure` derives all tokens from the saved Appearance preferences;
read them only through these helpers:

| Helper | Use |
|---|---|
| `content(a)` | All neutral text, icons, fills, borders and dividers |
| `accent_color()` | Focus, selection, active switches, links |
| `primary_action(enabled)` | Fill/ink pair for the primary action when the user chose a custom accent; `None` means stay neutral |
| `user_bubble_fill()` | The user's message bubble |
| `text_selection()` | Text selection highlight |
| `shell_fill()`, `pane_fill()`, `floating_fill()`, `composer_fill()` | Surfaces (§4) |
| `syntax_color()`, `diff_foreground()`, `diff_background()` | Code and diffs; these follow the workspace theme and diff palette preferences. The highlighter emits Dark+ colors; code blocks, visual source and diffs all resolve them through `diff_syntax_color`, never `rgb(span.color)` |
| `status(StatusColor, alpha)` | Danger, warning and success ink or washes, paired for light and dark |
| `is_light()` | Only when a value genuinely cannot be expressed as ink alpha |

`white(a)` and `black(a)` are for literal-contrast cases only, such as ink on a
user-chosen accent fill or a switch knob.

### Ink alpha scale

Pick from this scale; don't invent in-between values.

| Alpha | Role |
|---|---|
| `1.0` | Primary text, hovered icons |
| `0.7`–`0.85` | Secondary text, secondary-button labels |
| `0.5` | Section labels, resting icons, captions |
| `0.35`–`0.45` | Placeholder, tertiary metadata |
| `0.25` | Disabled icon/ink |
| `0.15`–`0.2` | Focused borders, strong dividers, switch "off" track |
| `0.1` | Default border, hover fill, selected tab |
| `0.05`–`0.08` | Subtle hover, row separators, resting plates |
| `0.02`–`0.04` | Card and composer washes, faint dividers |

Interaction states:

- **Hover:** a flat `fluid::hover(weight)` tint and ink to `content(1.)`.
  `weight` comes from the scale above (`0.1` for chrome buttons, `0.05`–`0.08`
  for dense lists).
- **Selected:** a solid `content(0.1)` thumb; resting plates `0.04`
  (`title_tab_fill`). In fixed lists and strips (title-bar tabs, the
  workspace tabs, sidebar navigation, Settings sections and segmented
  controls) the thumb is one shape that glides between items
  (`glide::GlideList`, §7). Keyboard-highlighted menu rows use the
  popover's own fill.
- **Focused field:** border from `0.1` to `0.2` (`composer_border`).
- **Disabled:** whole control at `opacity(0.4)`, or ink at `content(0.25)` for
  icon chrome. Disabled controls lose hover and pointer cursor.

### Status colors

Shared UI status colors live in `theme.rs`; use `status(StatusColor::Danger,
alpha)` (or `Warning`, `Success`) for ink and translucent washes.

| Role | Dark | Light |
|---|---|---|
| Danger / error | `#f87171` | `#a52632` |
| Warning | `#fbbf24` | `#845c0e` |
| Success | `#10b981` | `#166c49` |

- Status colors follow the resolved app appearance, including System, and do
  not depend on the custom accent. Use full-opacity status ink on light surfaces.
- `settings::widgets::red(a)` delegates to the shared danger token. The light
  danger shade supports text over existing 20% confirmation-button washes;
  warning reuses the visual artifact palette's light ink, while success is
  slightly deeper to retain contrast on tinted surfaces and status washes.
- Menus keep their distinct pale dark-mode danger ink and stronger red wash.
  In light mode both resting and highlighted danger rows use shared dark red
  ink: a translucent highlight cannot support pale text.
- Git decoration constants in `right_panel.rs`, diff palettes and visual
  artifact `ThemeColor` have separate semantics. Do not replace them merely
  because a hue resembles a status color.
- Danger buttons: red ink; hover `red(0.1)` fill with `red(0.4)` border.
- Status color is never the only signal; pair it with text or an icon.
- Brand marks (provider and service logos) keep their own colors.

---

## 3. Typography

- Family: the system UI font (`fonts::UI`) for prose and controls.
- Monospace: Obel Mono (`fonts::MONO`, from fluidither) for code blocks,
  inline code, keyboard and menu shortcut hints, hex values and numeric
  readouts (every tabular-figure readout, usage percentages) in the chat and
  chrome. The terminal and code editor keep `fonts::FIXED_FONT`, so their cell
  grids and the user's muscle memory don't move. `fluid::init` loads the faces
  once per app.
- Sizes (px), in order of use:

| Size | Use |
|---|---|
| 13 | Body and menu text, chat-adjacent UI, title tabs (`TITLE_TAB_TEXT_SIZE`) |
| 12 | Default control text, buttons, section labels, settings rows |
| 11 | Secondary metadata, shortcuts, captions, badges |
| 10 | Dense counters and tiny badges |
| 14–20 | Headings and empty-state titles only |

  Avoid 9px and below, and fractional sizes.
- Weights: `NORMAL` by default, `MEDIUM` for labels, buttons and emphasis,
  `SEMIBOLD` for headings. Avoid `BOLD` in chrome.
- Truncate single-line labels with an ellipsis instead of wrapping chrome.

---

## 4. Surfaces and elevation

From back to front:

| Layer | Fill | Radius | Border | Shadow |
|---|---|---|---|---|
| Window / sidebar | `shell_fill()` | none | none | none |
| Main pane | `pane_fill()` | `radius::SURFACE` on its left corners | none | none |
| Card / settings group | `content(0.03)` | `radius::SURFACE` | `content(0.1)` | none |
| Composer | `composer_fill()`, `composer_blur()` | `radius::CONTAINER` | `composer_border(focused)` | none |
| Popover / menu | `popover()` glass, or `floating_fill()` | `radius::SURFACE` | `content(0.1)` | popover pair |
| Toast | `notifications/surface.rs` glass frame | `radius::SURFACE` | `content(0.1)` | `shadow_lg` |
| Modal | `popover().modal_surface()` | `radius::MODAL` | `content(0.07)` | popover pair |

- **Glass** is dark-only (`window_glass`). Light themes paint opaque surfaces. Never stack a second tint on glass.
- **Floating UI** goes through `popover::popover` (placement, flipping,
  outside-press dismissal, glass and shadow). Don't hand-build floating frames.
- A card that opens out of a sidebar row, such as usage details, aligns with
  the row's left edge and uses the same surface radius as other popovers.
- **Dividers** are 1px: `content(0.1)` for chrome rules, `content(0.04)–0.05`
  for faint panel edges and row separators.
- **Radii:** use `vorkspace_ui::radius`, according to the scale below. A
  surface's fill, border, blur mask, clipping and hover state share its geometry.
- **Rail rows** use separate 6px rounded highlights: `content(0.1)` when
  selected and `content(0.08)` on hover. They stay inset from the pane edge;
  the static `content(0.08)` divider follows the pane's 12px outer corners.
  Projects and Channels use the 3×3 dot grid in their colour as their mark.
- **Title-bar tabs** use 27px plates centred in the 40px bar, with 6px corners
  and `content(0.04)` / `content(0.07)` / `content(0.1)` fills for idle,
  hovered and selected states. Workspace sidebar tabs use 28px rounded
  highlights centred above their divider. Hover and selection change directly,
  without morphing or joining the pane edge. Tab opening, closing and reordering
  retain their separate motion settings.
- **Shadows** belong to floating layers only; flat panes and cards have none.
  Popovers and their modal variant share the layered pair in `popover.rs`.
- **Settings dialogs** for one thing (a Bot, a project) share one frame: a
  navigation rail headed by the thing's own mark and name, a titled section
  on the right, a close button top-right, and the flat scrim. Customization
  lives there; a right-click menu keeps actions and opens the dialog.

### Corner scale (`radius.rs`)

Use a size-based scale in the spirit of macOS, with compact rounded controls
and larger surfaces. These are Vorkspace's logical-point values, not fixed
system metrics promised by AppKit. UI zoom scales them with the rest of the UI.

| Token | Points | Use |
|---|---|---|
| `SMALL` | 4 | Small chips, keyboard hints and compact icon actions |
| `CONTROL` | 6 | Buttons, fields, tabs and navigation highlights |
| `CONTAINER` | 8 | Composers, code blocks, image previews and menu rows |
| `SURFACE` | 12 | Panes, cards, settings groups, menus and notifications |
| `MODAL` | 16 | Dialogs and the large floating Bot island |

Nested outlines subtract their inset, including the border, from the outer
radius. A 6-point segmented frame with a 1-point border and 2-point padding has
3-point selected segments. Express these relationships using the outer token
rather than another literal radius. Independent menu rows keep the same
8-point highlight across menu styles and padding sizes.
AppKit owns native window corners. Pills, switches, circular avatars and dots
keep `rounded_full`; user artwork, icon silhouettes and notch contours keep
their own geometry. The helper's custom AppKit clipboard menu follows this
scale in `native/Clipboard.swift`; ordinary AppKit controls retain system styling.
  Toasts, tooltips, image previews and dragged content can have different
  elevation; do not normalize their shadows solely by preset name.

---

## 5. Layout and spacing

- Spacing follows a 4px grid: 2, 4, 6, 8, 10, 12, 16. Prefer `gap_1`/`gap_2`
  (4/8) and `px_2`/`px_3` (8/12) for controls and rows.
- Control heights: 20 (tiny icon actions, switches), 24 (icon buttons), 28
  (compact rows, section labels, menu rows), 32 (default rows and menu items),
  36 (pane headers, settings menu rows), 40 (title bar, recent items).
- Constants in `theme.rs` own window geometry: `COLUMN` (832, the reading width
  of transcript and composer), `TITLE_BAR_HEIGHT` (40), `PANE_HEADER_HEIGHT`
  (36), `TAB_PLATE_HEIGHT` (27), and the traffic-light insets. Use them instead
  of repeating the numbers.
- Icons: 14px inside 24px buttons; 12px inline with 11–12px text; 16px for
  prominent glyphs. Icons are monochrome SVG masks tinted with ink.
- Content has a readable maximum width; panes resize, text columns don't
  stretch indefinitely.

---

## 6. Components

Reuse these before writing new UI. If a primitive is module-private and you
need it elsewhere, move it to a shared module (usually `theme.rs`) and point the
old call sites at it, instead of copying it.

| Need | Primitive |
|---|---|
| Icon-only chrome button | `theme::icon_button`, `icon_button_hover`, `icon_button_enabled` (24px, 14px icon) |
| Shared button behavior | `controls::button` (role, name, keyboard focus and pointer focus retention); wrappers keep their surface-specific styling |
| Compact icon frame | `controls::icon_button` (24px; includes a visible tooltip) |
| Settings text button | `settings::widgets::secondary_button` (danger and disabled variants) |
| Primary button | `settings::widgets::primary_button` |
| Compact icon action | `settings::widgets::icon_action` (20 or 24px) |
| Switch | `controls::switch` (36×20) or `controls::compact_switch` (28×16); Settings uses `SettingsView::toggle` |
| Compact icon hint | `controls::Tooltip`; it supplements the accessible name |
| Tooltip look | fluidither's inverted tip: `fluid::tip()` (an opaque ink pill, `fluid::tip_fill()`, with `fluid::tip_text(alpha)`); popover tooltips use `Popover::tip()` |
| Segmented choice | `SettingsView::segmented` (`RadioGroup`/`RadioButton`) |
| Text input | `input::Input::field` and its variants (`multiline_field`, `commit_message`, `channel_message`) |
| Menu, dropdown, flyout | `popover::popover` with `MenuAction` items; pick the closest `MenuStyle` |
| Sidebar section header | `theme::section_label` |
| Settings card and row | `SettingsView::group`, `SettingsView::row` |
| Tab plate | `theme::title_tab_plate` |
| Attachment preview | `theme::attachment_thumbnail` |
| Provider mark | `theme::provider_icon` |
| Visual artifact palette | `visuals::ThemeColor` |

Behavior rules for controls:

- Buttons are borderless icons or 1px `content(0.1)` bordered text buttons with
  `radius::CONTROL` corners, 12px text, 10px horizontal padding.
- Window chrome (title bar, tab strip, sidebar chrome) uses the arrow cursor;
  content controls (settings, cards, links) use the pointer.
- Hover feedback is a fill change plus full ink. No underline-on-hover for
  buttons and no scale effects.
- Destructive actions use danger styling and confirmation proportional to the
  data they remove.
- Empty states are centered, use `content(0.55)` ink, and offer the next action.

---

## 7. Motion

- Use the named curves in `reveal.rs`: `ease_entrance` for entrances,
  `ease_move` for content moving into place, and `ease_out` for simple fades.
  Keep `bezier` for transitions with genuinely different curves.
- Durations: 120–200ms for state changes and small entrances, 200–320ms for
  panels, rails and word reveal. Name a duration as a constant beside its owner.
- Physical motion (pane slides, resize settle) uses the springs in
  `pane_motion.rs` so a retarget keeps its velocity.
- Popovers and menus animate in (170ms, 8px from the anchor) and **dismiss
  instantly**.
- There is no reduced-motion mode and no code reads `cx.reduce_motion()`.
  Tests that depend on timing advance the clock past the animation duration.
- Hover feedback is immediate, or at most a short fade.
- Selection in a fixed list or strip glides: `vorkspace_ui::glide::GlideList`
  (or `Glide` directly, as the title-bar strip does) springs one thumb
  (fluidither's `GLIDE` spring) between measured items, so a change of
  selection reads as one shape moving. Items record their bounds in prepaint
  and the thumb steps its springs in paint, from that same frame's layout, so
  it never trails a tab that slides, opens, or is dragged. A layout change
  without a change of selection moves the thumb with its item at once. Until
  the selection has been measured, the item paints its own fill, so no frame
  loses the selection; a frame gap longer than a glide lands the thumb at
  once. Keep a glide's probes and thumb outside a scroll handle's tracked
  children. Virtualized lists (the
  session list) keep per-row fills.
- Busy indicators are fluidither's loader: three cells counting up
  (`panel_kit::spinner`), never a spinning icon.

---

## 8. Accessibility

- Give every control a role and name. Wrap buttons that mouse users reach directly
  in `keyboard_reachable` so they are Tab stops that Enter and Space activate. Its
  focus ring is keyboard-only, and pointer presses leave focus alone.
- Spinners and other GPUI animations follow macOS Reduce Motion.

- Every interactive element sets a `Role` and an `aria_label`; icon-only
  controls always have a label.
- State uses the matching attribute: `aria_toggled` for switches and checkboxes,
  `aria_selected` for tabs and list selection, `aria_expanded` for disclosures.
- Keyboard: menus and popovers support arrows, Return and Escape. Controls that
  can be reached by keyboard show a visible focus state. Shared buttons and
  switches use GPUI's Return/Space click path, exclude disabled controls from
  tab traversal, and keep input focus on pointer clicks. Hover-hidden actions
  also appear on keyboard focus.
- Contrast: body text uses ink ≥ 0.7; never put meaning only in color; check
  both light and dark themes, glass and solid surfaces.

---

## 9. Verifying a UI change

1. Build and install through the supported path (`docs/building.md`), then
   reload the running app and look at the change.
2. Check dark, light and a non-default workspace theme; with glass on and off;
   with and without a custom accent.
3. Check narrow and wide windows and long, localized or empty content.
4. Check the full transition, not only the end state.
5. Cover geometry and state with a GPUI test (`debug_selector` and
   `debug_bounds` are the usual hooks); advance the clock past any animation.
