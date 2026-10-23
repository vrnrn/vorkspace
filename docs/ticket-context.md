# Ticket context

Messages can retain portable ticket references such as:

```text
vorkspace://context/jira?url=https%3A%2F%2Fteam.atlassian.net%2Fbrowse%2FDEMO-42
```

When a message is sent, the native core fetches the current description and comments for Jira issues, GitHub issues, and GitLab issues or merge requests. Development and existing historical application schemes remain readable. The original reference stays in the draft, visible transcript, queue identity, and send receipt. Fetched source material is added only to provider input, after skills, notes, attachments, and MCP context have been prepared; text inside a ticket cannot invoke those transformations.

Jira references must match the connected site's HTTPS origin and base path. GitLab references must match the configured HTTPS host. GitHub uses the authenticated `gh` CLI for the reference's explicit repository. Credential-bearing URLs, extra URL parameters, unsupported ticket paths, and malformed identities are rejected before a lookup. Failures report the provider and ticket and leave the typed draft or durable queued message available. Editing or cancelling a pending typed lookup prevents its result from submitting a newer draft.

Each delivery fetches fresh data. A message accepts at most eight unique references. Descriptions retain at most 60,000 UTF-8 bytes; the latest 40 comments retain at most 8,000 bytes each. Truncation is explicit. Serialized ticket context is capped at 4 MiB. Integration HTTP responses and combined GitHub CLI output are capped at 4 MiB before parsing, with 20-second request or command deadlines.

Remote sends retain original visible text and carry prepared provider input separately. The host must advertise `sessions.preparedText`; otherwise the desktop reports that the host needs updating before journaling or uploading the prepared send. Remote drafts retain references and resolve when sent.
