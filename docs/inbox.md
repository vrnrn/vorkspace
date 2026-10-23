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
