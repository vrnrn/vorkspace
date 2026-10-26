# Jira Cloud connector (ELI5)

Instead of constantly switching between your web browser and your code editor to read Jira tickets, Vorkspace can bring your **Jira Cloud** tickets right into its **Jira tab**.

For the full setup guide, see [Jira Cloud connector](../jira.md).

The Jira enable switch hides Jira without deleting its login details or filters.
You can also pick a default Bot, then choose another Bot or project for an
individual ticket before sending it to chat.

---

## 1. How to Connect

Go to **Settings → Connectors → Jira** and enter:
- Your Jira website address (like `yourcompany.atlassian.net`)
- Your work email
- An Atlassian API token (a special password you create in your Atlassian account settings)

Vorkspace saves that token in a private file on your computer that only your user account can read.

---

## 2. What You Can Do with Tickets

Once connected, click **Jira** in the sidebar:
- Read ticket descriptions and the latest 50 comments.
- Reply with a comment straight from Vorkspace.
- Click to **ask an AI agent** about the ticket or **start coding** on it—Vorkspace automatically hands the ticket's title, ID, and description to the agent so you don't have to copy-paste!

---

## 3. Two Ways to Filter Your Tickets

In Jira Settings, you can choose how Vorkspace picks which tickets to show:
- **By Project**: Simply check the boxes next to the Jira projects you care about.
- **By JQL (Jira Query Language)**: Write a custom search rule (like *"only show open tickets assigned to me in project ENG"*).

---

## 4. Automatic Agent Triggers

You can set up an **Automation** (`Jira → Issue appeared`) so that whenever a new matching ticket shows up in your Jira connector, an AI agent automatically starts investigating or preparing a fix in your local project!
