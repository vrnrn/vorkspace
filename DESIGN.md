# Vorkspace Design Guide

Vorkspace should feel like one quiet, native macOS tool. Chrome recedes; the
conversation, code and user's work carry the color. Use this guide for every UI
change, and extend it when you add a new shared convention.

Paths are relative to `crates/vorkspace-gpui/src/`.

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
| `syntax_color()`, `diff_foreground()`, `diff_background()` | Code and diffs; these follow the workspace theme and diff palette preferences |
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

- **Hover:** `content(0.1)` fill and ink to `content(1.)` (`theme::icon_button`).
  Dense lists may use `0.05`–`0.08`.
- **Selected:** `content(0.1)`; hovered `0.07`; resting `0.04`
  (`title_tab_fill`). Keyboard-highlighted menu rows use the popover's own fill.
- **Focused field:** border from `0.1` to `0.2` (`composer_border`).
- **Disabled:** whole control at `opacity(0.4)`, or ink at `content(0.25)` for
  icon chrome. Disabled controls lose hover and pointer cursor.

### Status colors

The app uses Tailwind-400 hues for status on dark surfaces:

| Role | Value | Current owner |
|---|---|---|
| Danger / error | `#f87171` | `settings/widgets.rs` `RED`, `red(a)` |
| Warning | `#fbbf24` | |
| Success | `#34d399` | |
| Info | `#38bdf8` | |

Status hues need a light-theme pair; `visuals.rs` `ThemeColor` is the reference
(for example Negative `#e76f73` dark / `#b9313d` light). Rules:

- Reuse the existing helper for a hue. When you need one outside its module,
  promote it to `theme.rs` with a light/dark pair rather than adding another
  local `const`.
- Danger buttons: red ink; hover `red(0.1)` fill with `red(0.4)` border.
- Status color is never the only signal; pair it with text or an icon.
- Brand marks (provider and service logos) keep their own colors.

---

## 3. Typography

- Family: the system UI font (`fonts::UI`). Monospace only for code, paths,
  terminals and diffs.
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
| Main pane | `pane_fill()` | none | 1px divider | none |
| Card / settings group | `content(0.03)` | `rounded_xl` | `content(0.1)` | none |
| Composer | `composer_fill()`, `composer_blur()` | 12 | `composer_border(focused)` | none |
| Popover / menu | `popover()` glass, or `floating_fill()` | 12 | `content(0.1)` | popover pair |
| Toast | `notifications/surface.rs` glass frame | 12 | `content(0.1)` | `shadow_lg` |
| Modal | `popover().modal_surface()` | 16 | `content(0.07)` | popover pair |

- **Glass** is dark-only (`window_glass`). Light themes paint opaque surfaces. Never stack a second tint on glass.
- **Floating UI** goes through `popover::popover` (placement, flipping,
  outside-press dismissal, glass and shadow). Don't hand-build floating frames.
- **Dividers** are 1px: `content(0.1)` for chrome rules, `content(0.04)–0.05`
  for faint panel edges and row separators.
- **Radii:** 4 for small chips and 20px icon actions; 6 (`rounded_md`) for
  buttons, icon buttons, inputs and tab plates; 8 for menu rows; 12 for
  floating surfaces and the composer; 16 for modals; `rounded_full` for pills,
  avatars, switches and dots.
- **Shadows** belong to floating layers only; flat panes and cards have none.

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
| Text button | `settings::widgets::secondary_button` (danger and disabled variants) |
| Primary button | `settings::widgets::primary_button` |
| Compact icon action | `settings::widgets::icon_action` (20 or 24px) |
| Switch | `SettingsView::toggle` (36×20, `Role::Switch`, `aria_toggled`) |
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
  `rounded_md`, 12px text, 10px horizontal padding.
- Window chrome (title bar, tab strip, sidebar chrome) uses the arrow cursor;
  content controls (settings, cards, links) use the pointer.
- Hover feedback is a fill change plus full ink. No underline-on-hover for
  buttons and no scale effects.
- Destructive actions use danger styling and confirmation proportional to the
  data they remove.
- Empty states are centered, use `content(0.55)` ink, and offer the next action.

---

## 7. Motion

- Easing comes from `reveal::bezier`. Use the established curves:
  - `(0.16, 1, 0.3, 1)`: entrances (popovers, reveals).
  - `(0.22, 1, 0.36, 1)`: content moving into place.
  - `(0, 0, 0.58, 1)`: simple fades and ease-out.
- Durations: 120–200ms for state changes and small entrances, 200–320ms for
  panels, rails and word reveal. Name a duration as a constant beside its owner.
- Physical motion (pane slides, resize settle) uses the springs in
  `pane_motion.rs` so a retarget keeps its velocity.
- Popovers and menus animate in (170ms, 8px from the anchor) and **dismiss
  instantly**.
- There is no reduced-motion mode and no code reads `cx.reduce_motion()`.
  Tests that depend on timing advance the clock past the animation duration.
- Hover feedback is immediate, or at most a short fade.

---

## 8. Accessibility

- Every interactive element sets a `Role` and an `aria_label`; icon-only
  controls always have a label.
- State uses the matching attribute: `aria_toggled` for switches and checkboxes,
  `aria_selected` for tabs and list selection, `aria_expanded` for disclosures.
- Keyboard: menus and popovers support arrows, Return and Escape. Controls that
  can be reached by keyboard show a visible focus state.
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
