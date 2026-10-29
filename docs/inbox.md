# Connectors

Vorkspace connectors support GitHub, GitLab, and Jira Cloud. Open **Settings → Connectors** to
enable the sources you use and manage their connections. GitHub uses the
GitHub CLI's active authentication. GitLab and Jira store connection details
locally; see [Jira Cloud connector](jira.md) for Jira setup and filtering.

Each source has an independent **Enable … connector** switch. Turning
one off removes its sidebar button and page, connection actions, unread counts, polling,
and automation trigger choices. Saved connection details, filters, read
state, and default Bot choices remain available when it is enabled again.
Disabling a source does not log out of GitHub CLI or delete credentials.

Enabled connectors each have a sidebar button in expanded and compact layouts.
Clicking a button opens that provider's page in the main pane. Like Search, Notes,
Automations and Bots, connector pages are sidebar pages: they hide the title strip
and are not saved as tabs. Authentication does not determine availability.
Each page remains pinned to its provider, including setup and failure states.

Connectors, Archive and Worktrees are grouped under App in Settings. The sidebar
options menu lists enabled connectors, Notes and Automations with visibility
switches and drag grips. Hiding a shortcut leaves its page state, connection and
background work available. Shortcut order is saved independently of title-tab
order and applies to both expanded and collapsed sidebars. Disabled connectors
retain their saved position. Search stays first; the Activity section and the
session-card density switch are retired because work is shown inline.

One retained owner fetches enabled providers and emits notification and automation
events once. Provider pages retain independent selection, filters, search, list
and detail scroll, detail requests and reply drafts while hidden or closed during
this run. Disabling a connector closes its page while retaining that state.
Provider filters are persisted alongside the existing native view state; legacy
shared filters seed each provider until it has its own saved choice. Connector and
Inbox tabs saved by older versions are dropped on restore. The old sidebar
visibility preference does not suppress individually enabled connectors.

The GitHub Code tab handles large diffs using the pull request's exact local
commit objects when available, or paginated file patches otherwise. Reconstruction
does not change the checkout or index. It retains up to 2 MiB of patches; the API
fallback lists up to 3,000 files. Omitted patches or incomplete results are marked
as truncated, and available hunks stay associated with their own file.

Jira and GitLab offer a **Default Bot** in Settings. It is preselected as the
destination when sending an item to chat. The destination menu also allows
choosing another Bot or local project for that item. A missing or unavailable
default falls back to a local project without deleting the saved choice.
Bot messages use the Bot's normal durable outbox.

After the first successful polling baseline, new items and newer remote
revisions can play one activity cue per batch. Project notification mutes,
category choices, active fetch/display filters, and the Sounds switch apply.
Startup history and changing the fetch query do not produce a cue. A successful
comment's server receipt or a PR action's returned revision suppresses only that
exact revision. Later remote changes remain unread and eligible for a cue, even
if they arrive before the own revision is polled. Missing receipt metadata is
treated conservatively and does not suppress an unknown revision.

Old desktop connector switches, Jira filters, and default Bot choices are read
from existing preferences. Subsequent changes are saved in the native settings
database. Opening the app does not rewrite or delete existing credential files.

All change reviews use **Unified** or **Side by side**. The choice is shared by
connector, working-tree, commit, and session reviews and is saved for the next launch.
Reviews show changed hunks with nearby context; there is no full-file diff mode.
Ordinary files still open in the file editor.
