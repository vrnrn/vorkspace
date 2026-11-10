import { modelsFor } from "../../models/sessions/models.js";
import {
  killChild,
  resolveClaudeBinary,
  spawnChild,
  watchChild,
  writeChild,
} from "../../protocol/child.js";
import { joinStreamText } from "../../protocol/streamText.js";
import { settleTextExchange } from "../../protocol/textExchange.js";
import {
  assistantTextBlocks,
  buildClaudeSpawnArgs,
  buildClaudeUserMessage,
  isClaudeInitMessage,
  parseJsonLine,
  streamDeltaFromEvent,
  turnStatusFromResult,
} from "./claudeProtocol.js";

const CHILD_ID = "vorkspace-claude-text";
const INIT_TIMEOUT_MS = 8_000;
const REQUEST_TIMEOUT_MS = 45_000;
let turns = Promise.resolve();

function pickTextModel() {
  const haiku = modelsFor("claude").find((model) =>
    /haiku/i.test(`${model.nativeId ?? ""} ${model.name} ${model.id}`),
  );
  return haiku?.nativeId ?? "claude-haiku-4-5";
}

/** One isolated Claude request for naming a session or branch. */
export function runClaudeTextPrompt(input) {
  const run = turns.then(() => runPrompt(input));
  turns = run.then(() => undefined, () => undefined);
  return run;
}

async function runPrompt({
  cwd,
  providerAccountId,
  prompt,
  timeoutMs = REQUEST_TIMEOUT_MS,
  signal,
}) {
  signal?.throwIfAborted();
  const { path } = await resolveClaudeBinary();
  signal?.throwIfAborted();
  let output = "";
  let finish;
  let fail;
  let ready = false;
  let exited = false;
  let wakeReady;
  watchChild(CHILD_ID, (line) => {
    const message = parseJsonLine(line);
    if (!message) return;
    if (isClaudeInitMessage(message)) {
      ready = true;
      wakeReady?.();
    }
    if (!finish) return;
    if (message.type === "assistant") {
      output = joinStreamText(output, assistantTextBlocks(message).join(""));
    } else if (message.type === "stream_event") {
      const delta = streamDeltaFromEvent(message);
      if (delta?.kind === "assistant") output += delta.text;
    } else if (message.type === "result") {
      const result = turnStatusFromResult(message);
      if (result.status === "failed") fail(new Error(result.error));
      else finish();
      finish = undefined;
    }
  }, () => {
    exited = true;
    wakeReady?.();
    fail?.(new Error("Claude text generator exited"));
  });

  try {
    await spawnChild(
      CHILD_ID,
      path,
      buildClaudeSpawnArgs({ isolated: true, model: pickTextModel() }),
      cwd,
      { provider: "claude", id: providerAccountId ?? "default" },
      "claude",
    );
    if (!ready && !exited) {
      await new Promise((resolve) => {
        const timer = setTimeout(resolve, INIT_TIMEOUT_MS);
        wakeReady = () => {
          clearTimeout(timer);
          resolve();
        };
      });
      wakeReady = undefined;
    }
    if (exited) throw new Error("Claude text generator exited");
    signal?.throwIfAborted();
    const completed = new Promise((resolve, reject) => {
      finish = resolve;
      fail = reject;
    });
    const message = JSON.stringify(buildClaudeUserMessage({ text: prompt }));
    await settleTextExchange(
      () => writeChild(CHILD_ID, message),
      completed,
      { label: "Claude", timeoutMs, signal },
    );
    if (!output.trim()) throw new Error("Claude returned empty output.");
    return output.trim();
  } finally {
    await killChild(CHILD_ID).catch(() => undefined);
  }
}
