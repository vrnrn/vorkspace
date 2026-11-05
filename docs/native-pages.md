# Native page design

Notes, Automations, the GitHub/GitLab/Jira Inbox, Todos and Search use the compact native page controls in `crates/vorkspace-gpui/src/controls/page.rs`. These helpers own presentation; each page retains its own data, navigation, editing and asynchronous work. Recording controls and meeting transcripts are part of Notes.

## Sizing and hierarchy

Dimensions are logical pixels and follow the app's UI scale. Pages inherit the app font and palette.

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
| Detail padding | 24 px; 16 px in narrow windows |
| Control and card corners | 6 px and 8 px |

Use gaps from 4, 8, 12, 16 and 24 px according to the hierarchy. Rows grow for their content; a fixed toolbar height does not constrain document titles or long Todo text.

Secondary text uses the palette's foreground at 60% opacity. Dividers use 10%, hover fills 5%, and selected rows 10%. Tabs use flat labels with a 2 px underline for the active section. Primary actions use the existing filled button style. Destructive actions and success or warning states use the theme's semantic colors. Provider marks retain their recognizable brand colors.

Filter fields blend into the toolbar when idle. The shared `search_frame` applies a 5% fill only while its input has focus, including keyboard focus. Opening a document page does not focus its filter. Ordinary form fields use `field_frame` and retain their visible input treatment. The Notes list uses the standard **+** toolbar action to create a note. Recording and audio source controls belong to the selected note's action row.

Use the shared action, icon action, primary, secondary, tab, row and empty-state helpers instead of reproducing their metrics in a page. Enabled controls participate in keyboard navigation and show focus. A disabled appearance does not replace validation in the action owner.

## Layout and continuity

Side-by-side lists and documents use `detail_pane` for the document's left edge, with the workspace's 12 px top and bottom corners. The list does not draw a second straight divider over that curve. In single-column layouts, the workspace frame supplies the outside corners.

Below a 900 px window viewport, Notes, Inbox and Automations show either the list or the selected detail. Back returns to the list while retaining the selection, draft and reading position. Resizing and unchanged recording refreshes do not replace the document or reset its scroll owner. Background capture updates leave a narrow window’s list open. Selecting Preview or Source while the recording attachment loads changes presentation without canceling the document load; separate load and presentation generations prevent stale loads and unwanted summary navigation. Direct and queued loads retain newer explicit tab choices over default or saved draft sections. The pending note identity and explicit tab stay together through discard, deletion, any required draft save, and queued loading. Repeated refreshes reuse that request; a failed discard or deletion can save retained edits before opening the selected attachment. Automation gallery cards become one column. Todos stacks its status columns. Search retains the same scopes and results, truncating long paths within the available width.

## Documents

Notes uses `markdown::Document` for authored Markdown and attached recording summaries. Inbox uses the same Markdown primitives and flowing-row styling while retaining its own measured, virtualized document and discussion list. Each page owns scrolling. Source editing uses the shared native input editor with the storage format's byte limit.

In Notes, **Preview** and **Source** display and edit authored note text. An attached recording adds **Summary** and **Transcript** tabs in the same row. **Edit summary** changes the Summary section to its Markdown editor while keeping Source dedicated to authored text. All four sections use flat tabs with an underline for the active section.

`notes.rs` owns note navigation, titles, tags and authored text; the embedded recording page owns capture state, transcript and summary drafts, edit leases and revision checks. SQLite keeps note content and the durable `note_recordings` relation, while recording files retain transcript and summary authority. Refresh and migration preserve user edits, and deletion tombstones prevent an old recording from recreating a deleted note. An untouched blank note or unnamed empty capture does not create a saved note.

The first dropped image materializes a new note with its image reference; failed imports leave an untouched draft unsaved. Recording-note deletion validates the capture owner and revision, then commits the note deletion and its tombstone before removing files. UI callbacks only update presentation. Notes loading retries file cleanup for tombstoned recordings, including interruption between the database commit and file removal.

Recording summaries contain the provider's completed final response after successful generation. Their renderer does not fetch images, interpret raw HTML, infer local file actions, or open arbitrary link schemes. HTTP, HTTPS and email links remain available. Authored Notes text and Inbox retain their existing attachment and project-link capabilities. Provider, model and prompt settings live in **Settings → Connectors → Notes**, whose Notes toggle also controls recording availability.

See [the plain-English companion](eli5/native-pages.md) for the visible behavior.
