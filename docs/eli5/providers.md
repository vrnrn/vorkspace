# Provider Setup (ELI5)

Vorkspace is the steering wheel, and **Providers** are the engines. Vorkspace doesn't sell you AI subscriptions—instead, it uses the AI command-line tools you already have installed and logged into on your computer.

For the exact install commands, see [Provider setup](../providers.md).

---

## 1. The 5 Supported Agents

You only need **one** of these installed to use Vorkspace, though you can install all five and switch between them anytime:
1. **Claude Code** (`claude`) — Anthropic's coding agent.
2. **Codex** (`codex`) — OpenAI's coding agent.
3. **Antigravity** (`agy`) — Google's coding agent (macOS & Linux).
4. **Pi** (`pi`) — Pi coding agent.
5. **omp** (`omp`) — omp coding agent.

When Vorkspace starts up, it looks to see which of these tools are installed on your computer. Any tool you haven't installed yet is simply grayed out with a helpful tip on how to install it.

---

## 2. Choosing Which Models You See

Some AI tools list dozens of different models. In **Settings → Providers**, you can click **Models** next to any agent and uncheck the models you never use so your model picker stays clean and uncluttered.

---

## 3. Switching Accounts When You Hit a Usage Limit

Your connected accounts’ usage appears together just above Settings in the sidebar. Click an account to see its limits and refresh them. The number on each row shows the limit closest to running out. The sidebar’s three-dot menu has a **Subscriptions** switch to hide or show this section; hiding it keeps your accounts and chats connected.

If you have more than one **Claude Code** or **Codex** account (for example, a personal account and a work account), you can add both in **Settings → Providers**:
- If one account runs out of messages in the middle of a task, a **Switch account** / **Choose another account** button appears right in the chat banner.
- When you pick your other account, Vorkspace copies the conversation's memory files (including subagents and undo history) over to the new account so you can keep working without starting over.

---

## 4. Bonus: Using GitHub Copilot Models Inside Claude Code

If you have a GitHub Copilot subscription, you have set `COPILOT_GITHUB_TOKEN` yourself (being logged in with `gh` is not enough on purpose), and Claude Code is installed, Vorkspace has a neat trick:
- It starts a small, private Rust translator on your computer that lets Claude Code use Copilot's GPT models (any GPT-6 or newer, like **GPT-6.1 Sol (1M)** and **GPT-6 Luna (1M)**) through your Copilot account!
- That translator stays with the running Claude process, including pauses between turns, and never alters your normal Claude settings.
- And if your company uses **AWS Bedrock** for Claude Code, Vorkspace uses that setup and shows **AWS Bedrock** in the sidebar's Subscriptions section. It won't ask you to sign into a Claude subscription or show made-up subscription percentages. Set up AWS access in Claude Code first, then restart Vorkspace.

On a remote machine, the host supplies its own JavaScript translator and uses that machine’s GitHub login.
