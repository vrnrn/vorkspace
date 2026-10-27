import { describe, expect, it, vi } from "vitest";
import { execFileSync } from "node:child_process";
import { mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import {
  listModels,
  prepareLaunch,
  readEvents,
  startGateway,
  toAnthropicEvents,
  toResponses,
} from "./claude-copilot.mjs";
async function collect(events) {
  const result = [];
  for await (const event of toAnthropicEvents(events, "copilot/gpt-6.1-sol"))
    result.push(event);
  return result;
}
const completed = {
  type: "response.completed",
  response: {
    status: "completed",
    usage: {
      input_tokens: 100,
      output_tokens: 12,
      input_tokens_details: { cached_tokens: 80 },
    },
  },
};
function toolStart(index, name = "Bash") {
  return {
    type: "response.output_item.added",
    output_index: index,
    item: {
      type: "function_call",
      id: `encrypted-start-${index}`,
      call_id: `call_${index}`,
      name,
    },
  };
}
function toolDone(index, args = "{}") {
  return {
    type: "response.output_item.done",
    output_index: index,
    item: {
      type: "function_call",
      id: `encrypted-done-${index}`,
      arguments: args,
    },
  };
}
function toolDelta(index, delta) {
  return {
    type: "response.function_call_arguments.delta",
    output_index: index,
    item_id: `encrypted-delta-${index}-${delta.length}`,
    delta,
  };
}
describe("Copilot Responses to Claude Messages", () => {
  it("launches Claude with session-local routing and exits with its child", () => {
    const dir = mkdtempSync(join(tmpdir(), "vorkspace-copilot-launch-"));
    const preload = join(dir, "catalog.mjs");
    const child = join(dir, "claude.mjs");
    writeFileSync(
      preload,
      `globalThis.fetch = async () => Response.json({data:[{
      id:'gpt-6-luna',name:'Fixture model',supported_endpoints:['/responses'],
      capabilities:{limits:{max_context_window_tokens:1000000,max_prompt_tokens:872000},supports:{reasoning_effort:['none','medium']}}
    }]});`,
    );
    writeFileSync(child, `console.log(JSON.stringify(process.argv.slice(2)));`);
    try {
      const output = execFileSync(
        process.execPath,
        [
          "--import",
          preload,
          fileURLToPath(new URL("./claude-copilot.mjs", import.meta.url)),
          process.execPath,
          child,
          "--model",
          "copilot/gpt-6-luna",
          "--effort",
          "none",
          "--settings",
          JSON.stringify({ disableAllHooks: true }),
        ],
        {
          encoding: "utf8",
          timeout: 10_000,
          env: { ...process.env, COPILOT_GITHUB_TOKEN: "fixture-token" },
        },
      );
      const args = JSON.parse(output);
      const settings = JSON.parse(args[args.indexOf("--settings") + 1]);
      expect(args).not.toContain("--effort");
      expect(settings.disableAllHooks).toBe(true);
      expect(settings.alwaysThinkingEnabled).toBe(false);
      expect(settings.env.CLAUDE_CODE_USE_BEDROCK).toBe("0");
      expect(settings.env.ENABLE_TOOL_SEARCH).toBe("false");
      expect(settings.env.ANTHROPIC_BASE_URL).toMatch(
        /^http:\/\/127\.0\.0\.1:/,
      );
      expect(settings.env.ANTHROPIC_AUTH_TOKEN).not.toBe("fixture-token");
    } finally {
      rmSync(dir, { recursive: true, force: true });
    }
  });
  it.each([true, false])(
    "keeps settings intact when effort comes first: %s",
    (effortFirst) => {
      const args = ["--model", "copilot/gpt-6-luna"];
      const settings = [
        "--settings",
        JSON.stringify({ disableAllHooks: true }),
      ];
      const effort = ["--effort", "none"];
      const launch = prepareLaunch(
        [
          ...args,
          ...(effortFirst
            ? [...effort, ...settings]
            : [...settings, ...effort]),
        ],
        [
          {
            id: "gpt-6-luna",
            contextWindow: 872_000,
            supportedEffortLevels: ["none", "medium"],
          },
        ],
      );
      expect(launch.args[launch.args.indexOf("--settings") + 1]).toBe(
        settings[1],
      );
      expect(launch.args).not.toContain("--effort");
      expect(launch.settings).toEqual({
        disableAllHooks: true,
        alwaysThinkingEnabled: false,
      });
      expect(launch.effort).toBe("none");
    },
  );
  it("redacts upstream credentials from gateway errors", async () => {
    const realFetch = globalThis.fetch;
    const credential = "fixture-upstream-credential";
    const gateway = await startGateway(credential);
    vi.stubGlobal(
      "fetch",
      vi.fn(async (_url, options) => {
        expect(options.redirect).toBe("error");
        expect(options.headers.Authorization).toBe(`Bearer ${credential}`);
        return Response.json(
          { error: { message: `Rejected ${credential}` } },
          { status: 401 },
        );
      }),
    );
    try {
      const result = await realFetch(`${gateway.url}/v1/messages`, {
        method: "POST",
        headers: { Authorization: `Bearer ${gateway.key}` },
        body: JSON.stringify({ model: "copilot/gpt-6-luna", messages: [] }),
      });
      const body = await result.text();
      expect(body).not.toContain(credential);
      expect(body).toContain("[redacted]");
    } finally {
      vi.unstubAllGlobals();
      gateway.close();
    }
  });
  it("does not advertise the normal context tier as a 1M variant", async () => {
    vi.stubGlobal(
      "fetch",
      vi.fn(async () =>
        Response.json({
          data: [
            {
              id: "gpt-6.1-sol",
              name: "GPT-6.1 Sol",
              supported_endpoints: ["/responses"],
              capabilities: {
                limits: {
                  max_context_window_tokens: 400_000,
                  max_prompt_tokens: 272_000,
                },
              },
            },
            {
              id: "gpt-6-luna",
              name: "GPT-6 Luna",
              supported_endpoints: ["/responses"],
              capabilities: {
                limits: {
                  max_context_window_tokens: 1_000_000,
                  max_prompt_tokens: 872_000,
                },
              },
            },
          ],
        }),
      ),
    );
    try {
      expect(await listModels("fixture-credential")).toEqual([
        { id: "gpt-6-luna", name: "GPT-6 Luna (1M)", contextWindow: 872_000 },
      ]);
    } finally {
      vi.unstubAllGlobals();
    }
  });
  it("lists any GPT model with major version 6 or newer and nothing older", async () => {
    const row = (id) => ({
      id,
      name: id,
      supported_endpoints: ["/responses"],
      capabilities: {
        limits: {
          max_context_window_tokens: 1_000_000,
          max_prompt_tokens: 872_000,
        },
      },
    });
    vi.stubGlobal(
      "fetch",
      vi.fn(async () =>
        Response.json({
          data: ["gpt-6-astra", "gpt-7", "gpt-5.6-luna", "kimi-k3"].map(row),
        }),
      ),
    );
    try {
      expect((await listModels("fixture-credential")).map((m) => m.id)).toEqual(
        ["gpt-6-astra", "gpt-7"],
      );
    } finally {
      vi.unstubAllGlobals();
    }
  });
  it("requires COPILOT_GITHUB_TOKEN instead of falling back to gh", async () => {
    const previous = process.env.COPILOT_GITHUB_TOKEN;
    delete process.env.COPILOT_GITHUB_TOKEN;
    const fetch = vi.fn();
    vi.stubGlobal("fetch", fetch);
    try {
      await expect(listModels()).rejects.toThrow("COPILOT_GITHUB_TOKEN");
      expect(fetch).not.toHaveBeenCalled();
    } finally {
      vi.unstubAllGlobals();
      if (previous !== undefined) process.env.COPILOT_GITHUB_TOKEN = previous;
    }
  });
  it("reserves the output budget inside each 1M context window", async () => {
    vi.stubGlobal(
      "fetch",
      vi.fn(async () =>
        Response.json({
          data: [
            {
              id: "gpt-6.1-sol",
              name: "GPT-6.1 Sol",
              supported_endpoints: ["/responses"],
              capabilities: {
                limits: {
                  max_context_window_tokens: 1_050_000,
                  max_prompt_tokens: 922_000,
                },
              },
            },
            {
              id: "gpt-6-luna",
              name: "GPT-6 Luna",
              supported_endpoints: ["/responses"],
              capabilities: {
                limits: {
                  max_context_window_tokens: 1_000_000,
                  max_prompt_tokens: 872_000,
                },
              },
            },
          ],
        }),
      ),
    );
    try {
      expect(await listModels("fixture-credential")).toEqual([
        { id: "gpt-6.1-sol", name: "GPT-6.1 Sol (1M)", contextWindow: 922_000 },
        { id: "gpt-6-luna", name: "GPT-6 Luna (1M)", contextWindow: 872_000 },
      ]);
    } finally {
      vi.unstubAllGlobals();
    }
  });
  it("honors disabled reasoning over Claude's inherited effort and thinking budget", () => {
    const converted = toResponses(
      {
        model: "copilot/gpt-6-luna",
        messages: [{ role: "user", content: "Reply with one word." }],
        output_config: { effort: "high" },
        thinking: { type: "enabled", budget_tokens: 10_000 },
      },
      "none",
    );
    expect(converted.reasoning).toEqual({ effort: "none" });
    expect(converted.contextTier).toBe("long_context");
  });
  it("closes a tool block with rotated IDs and complete argument JSON", async () => {
    const events = await collect([
      toolStart(0),
      toolDelta(0, '{"command":"printf '),
      toolDelta(0, 'héllo"}'),
      toolDone(0),
      completed,
    ]);
    const start = events.find((event) => event.type === "content_block_start");
    expect(start.content_block).toMatchObject({
      type: "tool_use",
      id: "call_0",
      name: "Bash",
    });
    const args = events
      .filter((event) => event.type === "content_block_delta")
      .map((event) => event.delta.partial_json)
      .join("");
    expect(JSON.parse(args)).toEqual({ command: "printf héllo" });
    expect(
      events.filter((event) => event.type === "content_block_stop"),
    ).toEqual([{ type: "content_block_stop", index: start.index }]);
    expect(
      events.findIndex((event) => event.type === "content_block_stop"),
    ).toBeLessThan(events.findIndex((event) => event.type === "message_delta"));
    expect(
      events.find((event) => event.type === "message_delta"),
    ).toMatchObject({
      delta: { stop_reason: "tool_use" },
      usage: {
        input_tokens: 20,
        cache_read_input_tokens: 80,
        output_tokens: 12,
      },
    });
  });
  it("keeps parallel tool arguments and closure events separate", async () => {
    const events = await collect([
      toolStart(2, "Read"),
      toolStart(3, "Bash"),
      toolDelta(3, '{"command":"pwd"}'),
      toolDelta(2, '{"file_path":"README.md"}'),
      toolDone(2),
      toolDone(3),
      completed,
    ]);
    const starts = events.filter(
      (event) => event.type === "content_block_start",
    );
    expect(starts.map((event) => event.content_block.name)).toEqual([
      "Read",
      "Bash",
    ]);
    for (const start of starts) {
      const args = events
        .filter(
          (event) =>
            event.type === "content_block_delta" && event.index === start.index,
        )
        .map((event) => event.delta.partial_json)
        .join("");
      expect(JSON.parse(args)).toEqual(
        start.content_block.name === "Read"
          ? { file_path: "README.md" }
          : { command: "pwd" },
      );
      expect(
        events.filter(
          (event) =>
            event.type === "content_block_stop" && event.index === start.index,
        ),
      ).toEqual([{ type: "content_block_stop", index: start.index }]);
    }
  });
  it("does not close a tool when a hidden reasoning item ends", async () => {
    const events = await collect([
      {
        type: "response.output_item.added",
        output_index: 0,
        item: { type: "reasoning", id: "reasoning-start" },
      },
      toolStart(1),
      {
        type: "response.output_item.done",
        output_index: 0,
        item: { type: "reasoning", id: "reasoning-done" },
      },
      toolDone(1, '{"command":"pwd"}'),
      completed,
    ]);
    expect(
      events
        .filter((event) => event.type === "content_block_start")
        .map((event) => event.content_block.type),
    ).toEqual(["tool_use"]);
    expect(
      events.filter((event) => event.type === "content_block_stop"),
    ).toEqual([{ type: "content_block_stop", index: 0 }]);
    expect(
      events.find((event) => event.type === "content_block_delta").delta
        .partial_json,
    ).toBe('{"command":"pwd"}');
  });
  it("reports truncated and failed streams instead of completing a partial answer", async () => {
    await expect(
      collect([toolStart(0), toolDelta(0, '{"command":')]),
    ).rejects.toThrow("without a completed response");
    await expect(
      collect([
        {
          type: "response.failed",
          response: { error: { message: "quota exceeded" } },
        },
      ]),
    ).rejects.toThrow("quota exceeded");
    const events = await collect([
      { type: "response.incomplete", response: { status: "incomplete" } },
    ]);
    expect(
      events.find((event) => event.type === "message_delta").delta.stop_reason,
    ).toBe("max_tokens");
  });
  it("preserves text order and images across a tool round trip", () => {
    const converted = toResponses({
      model: "copilot/gpt-6-luna",
      max_tokens: 512,
      system: [{ type: "text", text: "Inspect the image." }],
      messages: [
        { role: "user", content: "Read the file." },
        {
          role: "assistant",
          content: [
            {
              type: "thinking",
              thinking: "private provider state",
              signature: "anthropic-only",
            },
            { type: "text", text: "I will inspect it." },
            {
              type: "tool_use",
              id: "call_1",
              name: "Read",
              input: { file_path: "diagram.png" },
            },
          ],
        },
        {
          role: "user",
          content: [
            {
              type: "tool_result",
              tool_use_id: "call_1",
              content: [
                {
                  type: "image",
                  source: {
                    type: "base64",
                    media_type: "image/png",
                    data: "aW1hZ2U=",
                  },
                },
              ],
            },
            { type: "text", text: "Describe its labels." },
          ],
        },
      ],
    });
    expect(converted.model).toBe("gpt-6-luna");
    expect(converted.input).toEqual([
      {
        role: "user",
        content: [{ type: "input_text", text: "Read the file." }],
      },
      {
        role: "assistant",
        content: [{ type: "output_text", text: "I will inspect it." }],
      },
      {
        type: "function_call",
        call_id: "call_1",
        name: "Read",
        arguments: '{"file_path":"diagram.png"}',
      },
      {
        type: "function_call_output",
        call_id: "call_1",
        output: [
          { type: "input_image", image_url: "data:image/png;base64,aW1hZ2U=" },
        ],
      },
      {
        role: "user",
        content: [{ type: "input_text", text: "Describe its labels." }],
      },
    ]);
    expect(converted.instructions).toBe("Inspect the image.");
    expect(converted.store).toBe(false);
  });
  it("keeps tool failures distinct from successful output", () => {
    const converted = toResponses({
      model: "gpt-6.1-sol",
      messages: [
        {
          role: "user",
          content: [
            {
              type: "tool_result",
              tool_use_id: "call_2",
              is_error: true,
              content: "Permission denied",
            },
          ],
        },
      ],
    });
    expect(converted.input).toEqual([
      {
        type: "function_call_output",
        call_id: "call_2",
        output: "Tool error: Permission denied",
      },
    ]);
  });
  it("decodes UTF-8 and CRLF event frames across byte boundaries", async () => {
    const expected = {
      type: "response.output_text.delta",
      output_index: 0,
      delta: "é🦊",
    };
    const bytes = new TextEncoder().encode(
      `event: delta\r\ndata: ${JSON.stringify(expected)}\r\n\r\ndata: [DONE]\r\n\r\n`,
    );
    const events = [];
    for await (const event of readEvents(
      [...bytes].map((byte) => Uint8Array.of(byte)),
    ))
      events.push(event);
    expect(events).toEqual([expected]);
  });
  it("rejects unauthenticated local calls and unsupported models", async () => {
    const gateway = await startGateway("upstream-secret-must-not-leak");
    try {
      const denied = await fetch(`${gateway.url}/v1/messages`, {
        method: "POST",
        body: "{}",
      });
      expect(denied.status).toBe(401);
      expect(await denied.text()).not.toContain("upstream-secret");
      const unsupported = await fetch(`${gateway.url}/v1/messages`, {
        method: "POST",
        headers: {
          Authorization: `Bearer ${gateway.key}`,
          "Content-Type": "application/json",
        },
        body: JSON.stringify({ model: "not-an-enabled-model", messages: [] }),
      });
      expect(unsupported.status).toBe(400);
      expect(await unsupported.text()).toContain("Unsupported Copilot model");
    } finally {
      gateway.close();
    }
  });
});
