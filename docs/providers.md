# Provider setup

Install and sign in to at least one supported agent before using vorkspace. vorkspace runs the CLIs you install and reuses their existing logins.

Follow each provider’s linked installation guide for platform requirements and installation options. Use the installation and login commands below where provided.

- [Claude Code](https://claude.com/product/claude-code) - `claude auth login`
- [Codex](https://developers.openai.com/codex/cli) - `codex login`
- [Antigravity](https://antigravity.google/docs/cli-install) (macOS/Linux) - `curl -fsSL https://antigravity.google/cli/install.sh | bash`, then run `agy` once to sign in
- [Pi](https://pi.dev/) - `npm install -g @earendil-works/pi-coding-agent`
- [omp](https://omp.sh) - `curl -fsSL https://omp.sh/install | sh`

One provider is enough. vorkspace probes for each CLI at startup and disables the ones it can’t find, with a hint about how to install them, so a missing Codex doesn’t stop you from working on anything else.

Once a provider is installed and signed in, open a project folder in vorkspace, choose that agent, and send a message. For sessions on another machine, install and authenticate the providers on the host as described in [remote access](remote-access.md).

In **Settings → Providers**, use **Models** beside an agent to choose which models appear in pickers, favorites, and recent choices. Search for a model and uncheck it to hide it; **Show all** restores the list. The Global/project scope applies to model visibility too, and a model hidden globally stays hidden in every project. Existing conversations keep their selected model.

## Multiple accounts & usage-limit continuation

The **Subscriptions** section above Settings in the project sidebar shows usage across connected provider accounts. Each row opens its usage details and account controls; its summary shows the most-used limit window, and the details include every window. Use **Sidebar options → Subscriptions** to show or hide it. This preference is saved without disconnecting accounts or changing active conversations. Terminal commands remain available from the application menu and keyboard shortcuts.

In **Settings → Providers**, you can add multiple **Claude Code** and **Codex** accounts (each isolated under its own provider home directory) and switch the active account globally or per conversation.

When a Claude Code or Codex conversation (or a Bot using one of those providers) hits a rate or usage limit, vorkspace offers **Switch account** / **Choose another account** directly in the usage-limit banner:
- **Native context transfer**: For local sessions, vorkspace stops the limited CLI process, copies the conversation's native transcript/rollout files, subagent trees, file-history checkpoints, and closed subagent state into the newly selected account's home directory, and resumes the exact conversation byte-for-byte.
- **Transcript handoff fallback**: When native context cannot be copied directly (for example, when using a custom shared `CODEX_SQLITE_HOME`, a private Bot store, or a remote host), vorkspace carries the conversation forward using a structured transcript handoff on the next turn.

To run vorkspace from a checkout, see [building from source](building.md). See the [README](../README.md) for the native application overview.

## GitHub Copilot credentials

On macOS and Linux, desktop launches do not inherit your interactive shell environment. vorkspace forwards `COPILOT_GITHUB_TOKEN` from that shell to Claude processes that need it when the app environment lacks the variable. An inherited value takes precedence.

Export `COPILOT_GITHUB_TOKEN` in your shell configuration before you start vorkspace. Restart vorkspace after you change the token.

## Copilot GPT models in Claude Code

The desktop gateway runs in Rust alongside the installed Claude Code CLI. Remote hosts include their own Node gateway.
Export `COPILOT_GITHUB_TOKEN="$(gh auth token 2>/dev/null)"`, or sign in with `gh auth login`.
Restart vorkspace after you change the credential.
Select **Claude Code**, then select **GPT-6.1 Sol (1M)** or **GPT-6 Luna (1M)**.
Use the effort selector beside the model to select a supported level.

vorkspace lists these models only when the Copilot API reports access and a context window of at least one million tokens.
The adapter requests the long-context tier and uses the server's prompt budget for automatic compaction.
The prompt budget comes from the Copilot model catalog and reserves space for output.
The adapter rejects unavailable 1M variants instead of falling back to the normal tier.

The effort selector uses Copilot's advertised levels, with **Medium** as the default when available.
The adapter preserves **None** instead of passing it through Claude's incompatible effort handling.
Saved effort settings survive startup before model discovery finishes.
Claude-only options such as Fast Mode and Ultrathink do not appear for Copilot models.

Each Copilot session uses a private localhost adapter for the Anthropic Messages and Copilot Responses protocols.
The desktop adapter stays alive while its Claude process is parked between turns, and stops when that process is released. It uses a per-process secret, rejects browser origins, bounds requests and streamed frames, and does not change your saved Claude settings.
Ordinary Claude models retain your existing provider, including Bedrock.
Remote hosts use their JavaScript adapter and their own credentials.

Anthropic does not officially support non-Claude models in Claude Code.
The adapter supports text, images, streamed tool calls, and tool results.

When Claude Code reports Amazon Bedrock as its configured API provider, its row in **Subscriptions** displays **AWS Bedrock**. Subscription percentages and Claude sign-in prompts do not apply to that connection. Configure AWS credentials, region, and model access through [Claude Code's Bedrock setup](https://code.claude.com/docs/en/amazon-bedrock); vorkspace retains the CLI's settings and forwards the Bedrock environment from your login shell when needed.
