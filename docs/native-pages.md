# Native page design

Notes, Automations, the GitHub/GitLab/Jira Inbox, Todos and Search use the compact native page controls in `crates/vorkspace-gpui/src/controls/page.rs`. These helpers own presentation; each page retains its own data, navigation, editing and asynchronous work. Recording controls and meeting transcripts are part of Notes.

An automation editor saves against the revision it opened. Changes from another pane or window require Reset before the older editor can write or start a run. Typing during a save remains in the editor for the next save. Run now uses the same saved definition that was validated when the run receipt was written.

Inbox panes share the provider snapshot and related-session index from their window's Inbox owner. Each pane retains its own selected item, draft, filters and scroll position. A related session matches by URL or by work item kind, repository and number. The list preserves session order and shows each matching session once.

## Sizing and hierarchy

Dimensions are logical pixels and follow the app's UI scale. Pages inherit the app font, palette and `vorkspace_ui::radius` corner scale. Buttons and fields use `CONTROL`, code/editor containers use `CONTAINER`, cards and panes use `SURFACE`, and dialogs use `MODAL`.

| Element | Size |
| --- | --- |
| Toolbar | 36 px high |
| Button, search field, icon action frame | 28 px high |
| Action icon | 14 px |
| Page title | 20 px text, 26 px line height, semibold |
| Control or row label | 13 px text, 20 px line height |
| Secondary text | 12 px text, 18 px line height |
| Document editor and general body text | 14 px text, 22 px line height |
| Flowing Markdown prose | 14 px text, 24 px line height |
| List width | 280 px default; resizable lists clamp to 240–420 px |
| Detail content | At most 1024 px wide, centered |
| Detail padding | 24 px; 16 px in narrow pages or panes |
| Control, inset content and card corners | 6 px, 8 px and 12 px |

Use gaps from 4, 8, 12, 16 and 24 px according to the hierarchy. Rows grow for their content; a fixed toolbar height does not constrain document titles or long Todo text.

Secondary text uses the palette's foreground at 60% opacity. Dividers use 10%, hover fills 5%, and selected rows 10%. Tabs use flat labels with a 2 px underline for the active section. Primary actions use the existing filled button style. Destructive actions and success or warning states use the theme's semantic colors. Provider marks retain their recognizable brand colors.

Filter fields blend into the toolbar when idle. The shared `search_frame` applies a 5% fill only while its input has focus, including keyboard focus. Opening a document page does not focus its filter. Ordinary form fields use `field_frame` and retain their visible input treatment. The Notes list uses the standard **+** toolbar action to create a note. Recording and audio source controls belong to the selected note's action row.

Use the shared action, icon action, primary, secondary, tab, row and empty-state helpers instead of reproducing their metrics in a page. Enabled controls participate in keyboard navigation and show focus. A disabled appearance does not replace validation in the action owner.

## Settings

Settings uses Providers as the layout reference. Navigation groups appear in **App**, **Agents**, **Connectors** order. Disabled plugins keep their enable switch on Plugins, but their Settings button, navigation page and search results are hidden. Disabling a currently open plugin page returns to Plugins; stale direct navigation cannot reopen it until enabled. Each page has one title and a short description. Group headings distinguish related sets of controls; a single card does not repeat the page title or introduction. Row descriptions explain a setting's effect, limitation or prerequisite rather than restating its label.

`settings/widgets.rs` owns the card, group, row and selector styles: 12 px card corners, 16 px horizontal and 14 px vertical row padding, 13 px labels, 12 px supporting text, and 32 px between groups. Content is centered at up to 1024 px wide. Labels sit left of controls; rows stack in narrow panes while simple switches remain beside their labels. The Notes summary selectors use the same controls and account-scoped model catalog as Providers, retaining effort capabilities alongside model names.

## Layout and continuity

Pages and individual resources participate in the workspace tab strip and split layout. Separate note and connector item tabs keep independent editors and reading positions; reopening the same item focuses its existing pane. Pinned resource tabs omit the library list. Reopening a resource already tiled beside a project-bound pane restores that layout’s project and working copy before configuring the view. Authenticated workspace commands validate every sibling before reopening a layout. Standalone Search rebuilds its file index when the working copy changes, retains its query and scope, and opens file results against the directory that produced them. With an empty query, Search lists recents for the selected scope: started, unarchived conversations by last update, the most recently opened files that still exist in the index, and open projects. They use the same rows and keyboard navigation as results and follow session updates while the page is visible.

A Bot's document list opens workspace panes from a rounded floating island within the Bot pane; the sessions a Bot starts are listed in the rail's Sessions section with its name. The island collapses before it would compress the normal chat column: its breakpoint is the shared 832 px transcript/composer column plus the 316 px island lane, currently 1,148 logical pixels of Bot pane width. A top-right button or the Bot face opens the collapsed island. The compact overlay closes when a resource opens, on Escape, or when dismissed outside it. The island header shows the conversation owner's status and a settings icon. Documents show the three most recently updated; a Bot without documents shows one line instead. The chevron expands the list to the pane's bottom and makes it scrollable; the bottom chevron restores the compact view. The island uses pane opacity plus 20 percentage points, capped at 100%. Habits live in Bot settings. Bot appearance is edited only during creation or through the pencil on the settings avatar; the sidebar menu links to settings.

Project navigation stays on the left, and the collapsed rail and navigation strip use the same shell background. Workspace tools open from the sidebar, shown with the top bar's folder toggle; hovering over a rail icon or the rail edge never opens a sidebar preview. All content panes use the existing split limits and keyboard navigation.

Side-by-side lists and documents use `detail_pane` for the document's left edge, with the workspace's 12 px top and bottom corners. The list does not draw a second straight divider over that curve. In single-column layouts, the workspace frame supplies the outside corners. Resizable note and connector lists highlight the detail pane’s curved border while hovered or dragged. Their wider drag target stays transparent.

Notes, Inbox and Automations use each pane’s allocated width for their 900 px single-column breakpoint, or the window viewport when shown standalone. Back returns to the list while retaining the selection, draft and reading position. Resizing and unchanged recording refreshes do not replace the document or reset its scroll owner. Background capture updates leave a narrow page’s list open. Selecting Preview or Source while the recording attachment loads changes presentation without canceling the document load; separate load and presentation generations prevent stale loads and unwanted summary navigation. Direct and queued loads retain newer explicit tab choices over default or saved draft sections. The pending note identity and explicit tab stay together through discard, deletion, any required draft save, and queued loading. Repeated refreshes reuse that request; a failed discard or deletion can save retained edits before opening the selected attachment. Automation gallery cards become one column. To-do columns wrap to the actual tile width, while sidebar boards stack vertically. Search retains the same scopes and results, truncating long paths within the available width.

## Documents

Notes uses `markdown::Document` for authored Markdown and attached recording summaries. Inbox uses the same Markdown primitives and flowing-row styling while retaining its own measured, virtualized document and discussion list. Each page owns scrolling. Source editing uses the shared native input editor with the storage format's byte limit.

In Notes, **Preview** and **Source** display and edit authored note text. An attached recording adds **Summary** and **Transcript** tabs in the same row. **Edit summary** changes the Summary section to its Markdown editor while keeping Source dedicated to authored text. All four sections use flat tabs with an underline for the active section.

`notes.rs` owns note navigation, titles, tags and authored text; the embedded recording page owns capture state, transcript and summary drafts, edit leases and revision checks. SQLite keeps note content and the durable `note_recordings` relation, while recording files retain transcript and summary authority. Refresh and migration preserve user edits, and deletion tombstones prevent an old recording from recreating a deleted note. An untouched blank note or unnamed empty capture does not create a saved note.

The first dropped image materializes a new note with its image reference; failed imports leave an untouched draft unsaved. Recording-note deletion validates the capture owner and revision, then commits the note deletion and its tombstone before removing files. UI callbacks only update presentation. Notes loading retries file cleanup for tombstoned recordings, including interruption between the database commit and file removal.

Recording summaries contain the provider's completed final response after successful generation. Their renderer does not fetch images, interpret raw HTML, infer local file actions, or open arbitrary link schemes. HTTP, HTTPS and email links remain available. Authored Notes text and Inbox retain their existing attachment and project-link capabilities. Provider, model, effort and prompt settings live in **Settings → Connectors → Notes**, whose Enable notes toggle also controls recording availability.

See [the plain-English companion](eli5/native-pages.md) for the visible behavior.
