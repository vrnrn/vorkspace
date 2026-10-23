# Inbox connectors

Inbox supports GitHub, GitLab, and Jira Cloud. Open **Settings → Inbox** to
enable the sources you use and manage their connections. GitHub uses the
GitHub CLI's active authentication. GitLab and Jira store connection details
locally; see [Jira Cloud inbox](jira.md) for Jira setup and filtering.

Each source has an independent **Enable … inbox connector** switch. Turning
one off removes its Inbox tabs, connection actions, unread counts, polling,
and automation trigger choices. Saved connection details, filters, read
state, and default Bot choices remain available when it is enabled again.
Disabling a source does not log out of GitHub CLI or delete credentials.

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
Startup history, changing the fetch query, and the next revision caused by a
successful comment or action in the app do not produce a cue.

Old desktop connector switches, Jira filters, and default Bot choices are read
from existing preferences. Subsequent changes are saved in the native settings
database. Opening the app does not rewrite or delete existing credential files.

All change reviews use **Unified** or **Side by side**. The choice is shared by
Inbox, working-tree, commit, and session reviews and is saved for the next launch.
Reviews show changed hunks with nearby context; there is no full-file diff mode.
Ordinary files still open in the file editor.
