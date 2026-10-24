# Inbox connectors

Inbox brings GitHub, GitLab, and Jira Cloud work into Vorkspace. In
**Settings → Inbox**, turn on the services you use and turn off the others.

Turning a service off hides it and stops checking it. Your saved login details
and choices stay on your computer, so you can turn it back on later.
**Disconnect** is a separate action for removing a saved connection.

You can review large GitHub pull requests in the Code tab even when GitHub refuses
to send all changes together. Vorkspace reads the changes from your local Git
history when possible, or loads them in smaller groups from GitHub. Your checked
out files stay unchanged. If some changes cannot be shown, it says so and keeps
the available changes visible.

For Jira and GitLab, you can pick a default Bot to receive tickets or changes.
Before sending an item to chat, you can choose another Bot or project instead.
If the saved Bot is unavailable, Vorkspace lets you use a project.

Inbox can play a quiet sound when new work or an update arrives. It stays quiet
for old work when the app opens, muted projects, hidden items, and updates it can
identify as your own successful comments or actions. Someone else's later update
still counts as new activity. If a service cannot identify your update, the app
keeps notifications enabled for it. You can turn sounds off in Settings.

Your existing desktop preferences are kept when using the native app.
See the [technical guide](../inbox.md) or [Jira guide](jira.md) for details.

Changes have two views everywhere: **Unified** puts removed and added lines in
one column; **Side by side** puts the old version on the left and the new version
on the right. Your choice is remembered. Reviews focus on the changes with nearby
lines, while opening a file normally still gives you the file editor.
