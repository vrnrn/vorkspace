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

If you have more than one **Claude Code** or **Codex** account (for example, a personal account and a work account), you can add both in **Settings → Providers**:
- If one account runs out of messages in the middle of a task, a **Switch account** / **Choose another account** button appears right in the chat banner.
- When you pick your other account, Vorkspace copies the conversation's memory files (including subagents and undo history) over to the new account so you can keep working without starting over.

---

## 4. Bonus: Using GitHub Copilot Models Inside Claude Code

If you have a GitHub Copilot subscription (`gh auth login` or `COPILOT_GITHUB_TOKEN`) and Claude Code installed, Vorkspace has a neat trick:
- It starts a tiny, private translator on your computer (`host/claude-copilot.mjs`) that lets Claude Code use **GPT-6.1 Sol (1M)** and **GPT-6 Luna (1M)** through your Copilot account!
- That translator only runs while your chat is open and never alters your normal Claude settings.
- And if your company uses **AWS Bedrock** for Claude Code, Vorkspace detects that automatically and shows an **AWS Bedrock** badge.
