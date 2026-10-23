# Ticket context

A ticket reference is a link to a Jira, GitHub, or GitLab issue, or a GitLab merge request. The app keeps that link in your message and reads the latest description and discussion when you send it. It gives the agent that information separately, so ticket text cannot accidentally select local files or commands.

The link contains no password or saved ticket body. You need access through the connected integration. A failed lookup keeps your draft or queued message available, and typing something newer stops an older lookup from sending it. Large tickets have clear limits and indicate omitted content.

Remote hosts need support for prepared ticket input. If a host is too old, the app asks you to update it before sending the reference. Saving a remote draft keeps its link for a fresh lookup when you send it.
