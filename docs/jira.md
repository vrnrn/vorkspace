# Jira Cloud connector

Open **Settings → Connectors → Jira**, enter your Jira site, Atlassian account email,
and an API token **without scopes**, then select **Connect**. The connection is
validated before credentials are saved. Create a token from
[your Atlassian account](https://id.atlassian.com/manage-profile/security/api-tokens).
Scoped tokens and Jira Data Center are not supported by this integration.

The **Enable Jira connector** switch hides Jira and stops polling while
keeping credentials and filters. **Default Bot** preselects a chat destination;
you can choose a different Bot or local project before sending a ticket.

Open the Jira page from the sidebar to browse issues, read descriptions and comments,
post comments, ask about a ticket, or start work in a local project. Starting work
and asking about a ticket include its description and Jira identifier.

Choose **Project** or **JQL** from the **Filter tickets by** dropdown in Jira settings:

- **Project** keeps the existing project checklist. Selections are shared with
  the Jira filter menu. Unchecked projects are excluded from fetching and
  background notifications. The Jira connector loads up to 40 open issues or 100 issues
  when including closed history, ordered by last update. An unfiltered
  all-status query covers the last year of activity.
- **JQL** fetches tickets using your query, for example
  `project = ENG AND statusCategory != Done ORDER BY updated DESC` or
  `filter = 12345`. Select **Apply JQL** to check the query with Jira and save it.
  Invalid queries leave the previous filter active. The query replaces project,
  assignment, and status fetch constraints and loads up to 100 matching tickets.
  Its ordering is sent to Jira unchanged; the Jira page displays tickets by last
  update. Jira status, time, and text filters can still narrow displayed results.

Selecting JQL hides Jira project rows from **Project notifications** at the top
of **Connectors** settings. Switching back to Project restores the rows and
their notification preferences. Other providers' projects remain available.

The saved mode and query also apply to background polling and Jira automation
triggers. Switching back to Project restores your project selections; switching
to JQL and applying it again restores your saved query. Long comment threads show
the latest 50 comments, with a link to Jira.

Automations offer **Jira → Issue appeared** after connecting. They run in the
automation's selected local project when a Jira issue first appears in the polled
connector, after its initial snapshot. The project-key filter uses values such as
`ENG`. This is polling, not a webhook for every issue created on the site.

**Disconnect** removes the saved credentials and clears cached Jira content.
Credentials are stored in the app's local data directory; on Unix the file is
created with owner-only permissions.
