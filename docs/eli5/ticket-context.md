# Ticket Context Links (ELI5)

When you want an AI agent to work on a bug report or feature request from **Jira**, **GitHub Issues**, or **GitLab Issues / Merge Requests**, copying and pasting the title, description, and long comment threads by hand is tedious.

**Ticket Context** turns any ticket into a magic card you can drop straight into a chat.

For the full protocol details, see [Ticket context](../ticket-context.md).

---

## 1. Two Easy Ways to Share a Ticket with an Agent

1. **Send to Chat**:
   - Click **Send to chat** on an issue or merge request and pick which **Project** or **Mono** should get it.
   - Vorkspace attaches a neat ticket card to the chat box and waits for you to type any extra instructions before pressing Send.
   - You can even pick a **Default Mono** in **Settings → Integrations** so your favorite personal agent is preselected automatically!

2. **Copy Context (`vorkspace://context/...`)**:
   - Click **Copy context** to copy a short link like `vorkspace://context/jira?url=...`.
   - When you paste that link into any Vorkspace chat and hit Send, Vorkspace automatically fetches the latest description and comments behind the scenes and hands them to the AI agent.

---

## 2. Why These Links Are Safe to Share

- **No Secrets Inside**: A `vorkspace://context/...` link only holds the ticket's web address—never your passwords, API tokens, or private ticket text.
- **Always Fresh**: Because Vorkspace looks up the ticket right when you press Send, the agent always reads the newest comments.
- **Bounded Size**: You can attach up to 8 tickets in one message, and super long comment threads are trimmed automatically so they don't overflow the agent's memory.
