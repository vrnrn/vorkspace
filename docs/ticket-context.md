# Ticket context

Jira, GitHub and GitLab issues, and GitLab merge requests, offer **Send to chat**, a searchable destination picker with **Projects** and **Bots**, and **Copy context**. Opening a destination attaches a ticket card and leaves the message unsent. A Bot keeps its conversation and typed draft; another pending ticket must be sent or removed first.

Choose a **Default Bot** for Jira and GitLab in Settings → Integrations. **Send to chat** preselects that Bot, and you can choose another Bot or project before sending. Choose **No default Bot** to keep the usual destination selection. If the saved Bot is no longer available, the picker falls back to an available project or Bot. Sending attaches the ticket to the chosen chat and leaves the message unsent.

Copy context writes a reference such as:

```text
vorkspace://context/jira?url=https%3A%2F%2Fdemo.atlassian.net%2Fbrowse%2FDEMO-42
```

Paste it into a project or Bot message, including a remote project chat. When submitted, vorkspace fetches the current description and comments using the connected integration. Queued Bot messages resolve when delivered. The visible message retains the reference; provider input receives the source material separately. Failed lookups report an error rather than sending incomplete context. These links work for other vorkspace installations with access to the same ticket; they contain no credentials or copied ticket body. Pasting into an unrelated application stores the link; that application cannot resolve vorkspace context itself.

Opening a native link brings vorkspace forward and opens a new unsent project chat with the ticket attached. Markdown links in vorkspace open the same flow. Native protocol registration ships in the desktop bundle; macOS needs a rebuilt installed bundle to handle links from other applications. No message is automatically submitted by opening a link.

Jira links must match the connected site's origin and base path. GitLab links must match the connected host. A merge request link ends in `/-/merge_requests/<number>`. The fetched context for a merge request also lists its source and target branches. GitHub uses the authenticated GitHub CLI for the explicit repository. Lookups fail if the account cannot access the ticket. At most eight unique tickets resolve per message; large descriptions and comment histories are bounded and marked as truncated.

The implementation is shared: `ticketContext.ts` owns portable references and provider adapters, `ticketChat.ts` owns Bot attachment, and `TicketContextActions.tsx` owns destination selection and clipboard feedback. Add another provider's reference parser and fetch adapter to extend the same flow. GitHub pull requests and other inbox sources keep their existing actions.

Dummy Jira, GitHub and GitLab data cover this flow in tests; no live tracker account is needed. Open `/tests/browser/jira-context.html` on the development server for the interactive fixture.
