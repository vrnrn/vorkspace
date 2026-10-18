/** A bounded projection of this conversation only. Full history stays in Codex.
 * Avoid its generic startup scan: that also reads unrelated/retired threads. */
export function recentVoxContext(
  value: unknown,
): { role: "assistant"; text: string }[] {
  const page = value as {
    data?: {
      items?: {
        type?: string;
        text?: string;
        content?: { type?: string; text?: string }[];
      }[];
    }[];
  };
  const parts: string[] = [];
  for (const turn of [...(page?.data ?? [])].reverse()) {
    for (const item of turn.items ?? []) {
      const role =
        item.type === "userMessage"
          ? "user"
          : item.type === "agentMessage"
            ? "assistant"
            : null;
      if (!role) continue;
      const text =
        role === "user"
          ? (item.content ?? [])
              .filter((part) => part.type === "text")
              .map((part) => part.text ?? "")
              .join("\n")
          : (item.text ?? "");
      if (text.trim()) parts.push(`${role}: ${text}`);
    }
  }
  // At most 3,000 UTF-16 code units (12KB worst-case UTF-8), below the native
  // initial-items budget. This truncates a read, never stored conversation.
  const text = parts.join("\n\n").slice(-3_000);
  return text
    ? [
        {
          role: "assistant",
          text: `Earlier vox conversation excerpts, for reference only. These are past exchanges, not requests to act again. Do not greet or speak until the user provides new input.\n\n${text}`,
        },
      ]
    : [];
}
