import { spawn } from "node:child_process";
import { randomBytes, randomUUID, timingSafeEqual } from "node:crypto";
import { once } from "node:events";
import { createServer } from "node:http";
import { pathToFileURL } from "node:url";

const API = "https://api.githubcopilot.com";
// Any GPT model whose major version is 6 or newer; gpt-5.6-* stays excluded.
const supported = (id) => {
  const major = /^gpt-(\d+)(?:[.-]|$)/.exec(id ?? "")?.[1];
  return major !== undefined && Number(major) >= 6;
};

// Copilot access is opt-in: a gh login alone must not enable it.
function token() {
  const inherited = process.env.COPILOT_GITHUB_TOKEN?.trim();
  if (inherited) return inherited;
  throw new Error("Export COPILOT_GITHUB_TOKEN to use Copilot models.");
}

function headers(credential) {
  return {
    Authorization: `Bearer ${credential}`,
    "Content-Type": "application/json",
    "Copilot-Integration-Id": "vscode-chat",
    "Editor-Version": "vscode/1.104.0",
    "Editor-Plugin-Version": "copilot-chat/0.35.0",
    "X-GitHub-Api-Version": "2026-06-01",
  };
}

function redact(message, ...secrets) {
  return secrets
    .filter(Boolean)
    .reduce(
      (text, secret) => text.replaceAll(secret, "[redacted]"),
      String(message),
    );
}

async function responseError(response, credential) {
  const body = await response.json().catch(() => null);
  return redact(
    body?.error?.message ??
      body?.message ??
      `Copilot returned HTTP ${response.status}.`,
    credential,
  );
}

export async function listModels(credential = token()) {
  const response = await fetch(`${API}/models`, {
    headers: headers(credential),
    redirect: "error",
    signal: AbortSignal.timeout(10_000),
  });
  if (!response.ok) throw new Error(await responseError(response, credential));
  const { data } = await response.json();
  if (!Array.isArray(data))
    throw new Error("Copilot returned an invalid model catalog.");
  return data
    .filter(
      (model) =>
        supported(model.id) &&
        model.supported_endpoints?.includes("/responses") &&
        model.capabilities?.limits?.max_context_window_tokens >= 1_000_000 &&
        Number.isSafeInteger(model.capabilities?.limits?.max_prompt_tokens) &&
        model.capabilities.limits.max_prompt_tokens > 0 &&
        (!model.policy || model.policy.state === "enabled"),
    )
    .map((model) => ({
      id: model.id,
      name: `${model.name} (1M)`,
      contextWindow: model.capabilities?.limits?.max_prompt_tokens,
      ...(Array.isArray(model.capabilities?.supports?.reasoning_effort)
        ? {
            supportedEffortLevels: model.capabilities.supports.reasoning_effort,
          }
        : {}),
    }));
}

function modelId(model) {
  const id = model?.replace(/^copilot\//, "");
  if (!supported(id)) throw new Error(`Unsupported Copilot model: ${model}`);
  return id;
}

function textBlocks(content) {
  if (typeof content === "string") return content;
  return (content ?? [])
    .filter((block) => block.type === "text")
    .map((block) => block.text)
    .join("\n");
}

function inputBlock(block, role) {
  if (block.type === "text") {
    return {
      type: role === "assistant" ? "output_text" : "input_text",
      text: block.text,
    };
  }
  if (block.type === "image") {
    const source = block.source;
    return {
      type: "input_image",
      image_url:
        source.type === "url"
          ? source.url
          : `data:${source.media_type};base64,${source.data}`,
    };
  }
  // Claude tool search returns references. All schemas are sent to Copilot.
  if (block.type === "tool_reference") {
    return { type: "input_text", text: `Available tool: ${block.tool_name}` };
  }
  throw new Error(`Unsupported Copilot content: ${block.type}`);
}

export function toResponses(body, effort = body.output_config?.effort) {
  const input = [];
  for (const message of body.messages) {
    const blocks =
      typeof message.content === "string"
        ? [{ type: "text", text: message.content }]
        : message.content;
    let content = [];
    const flush = () => {
      if (content.length) input.push({ role: message.role, content });
      content = [];
    };
    for (const block of blocks) {
      if (block.type === "thinking" || block.type === "redacted_thinking")
        continue;
      if (block.type === "tool_use") {
        flush();
        input.push({
          type: "function_call",
          call_id: block.id,
          name: block.name,
          arguments: JSON.stringify(block.input),
        });
      } else if (block.type === "tool_result") {
        flush();
        const output =
          typeof block.content === "string"
            ? block.content
            : (block.content ?? []).map((part) => inputBlock(part, "user"));
        input.push({
          type: "function_call_output",
          call_id: block.tool_use_id,
          output: block.is_error
            ? `Tool error: ${typeof output === "string" ? output : JSON.stringify(output)}`
            : output,
        });
      } else {
        content.push(inputBlock(block, message.role));
      }
    }
    flush();
  }
  const result = {
    model: modelId(body.model),
    input,
    store: false,
    contextTier: "long_context",
  };
  if (body.system) result.instructions = textBlocks(body.system);
  if (body.max_tokens) result.max_output_tokens = body.max_tokens;
  if (body.tools?.length) {
    result.tools = body.tools.map((tool) => {
      if (!tool.input_schema)
        throw new Error(`Unsupported Copilot tool: ${tool.name ?? tool.type}`);
      return {
        type: "function",
        name: tool.name,
        description: tool.description,
        parameters: tool.input_schema,
        strict: false,
      };
    });
  }
  const choice = body.tool_choice;
  if (choice?.type === "tool")
    result.tool_choice = { type: "function", name: choice.name };
  else if (choice)
    result.tool_choice = choice.type === "any" ? "required" : choice.type;
  if (choice?.disable_parallel_tool_use) result.parallel_tool_calls = false;
  if (effort) result.reasoning = { effort };
  else if (body.thinking?.type === "enabled") {
    const budget = body.thinking.budget_tokens;
    result.reasoning = {
      effort: budget >= 10_000 ? "high" : budget >= 5_000 ? "medium" : "low",
    };
  }
  const format = body.output_config?.format ?? body.output_format;
  if (format?.type === "json_schema") {
    result.text = {
      format: {
        type: "json_schema",
        name: "response",
        schema: format.schema,
        strict: false,
      },
    };
  }
  return result;
}

function usage(value) {
  const cached = value?.input_tokens_details?.cached_tokens ?? 0;
  return {
    input_tokens: (value?.input_tokens ?? 0) - cached,
    output_tokens: value?.output_tokens ?? 0,
    cache_read_input_tokens: cached,
  };
}

function stopReason(response, tools) {
  return response.status === "incomplete"
    ? "max_tokens"
    : tools
      ? "tool_use"
      : "end_turn";
}

function toMessage(response, model) {
  const content = response.output.flatMap((item) => {
    if (item.type === "function_call") {
      return [
        {
          type: "tool_use",
          id: item.call_id,
          name: item.name,
          input: JSON.parse(item.arguments),
        },
      ];
    }
    return item.type === "message"
      ? item.content
          .filter((part) => part.type === "output_text")
          .map((part) => ({ type: "text", text: part.text }))
      : [];
  });
  return {
    id: `msg_${randomUUID()}`,
    type: "message",
    role: "assistant",
    model,
    content,
    stop_reason: stopReason(
      response,
      content.some((part) => part.type === "tool_use"),
    ),
    stop_sequence: null,
    usage: usage(response.usage),
  };
}

export async function* readEvents(body) {
  const decoder = new TextDecoder();
  let buffer = "";
  for await (const chunk of body) {
    buffer +=
      typeof chunk === "string"
        ? chunk
        : decoder.decode(chunk, { stream: true });
    let match;
    while ((match = /\r?\n\r?\n/.exec(buffer))) {
      const frame = buffer.slice(0, match.index);
      buffer = buffer.slice(match.index + match[0].length);
      const data = frame
        .split(/\r?\n/)
        .filter((line) => line.startsWith("data:"))
        .map((line) => line.slice(5).trimStart())
        .join("\n");
      if (data && data !== "[DONE]") yield JSON.parse(data);
    }
  }
  if (buffer.trim())
    throw new Error("Copilot ended an incomplete event frame.");
}

export async function* toAnthropicEvents(events, model) {
  yield {
    type: "message_start",
    message: {
      id: `msg_${randomUUID()}`,
      type: "message",
      role: "assistant",
      model,
      content: [],
      stop_reason: null,
      stop_sequence: null,
      usage: { input_tokens: 0, output_tokens: 0 },
    },
  };
  // Copilot rotates item IDs between events. output_index identifies the item.
  const blocks = new Map();
  let nextIndex = 0;
  let tools = false;
  for await (const event of events) {
    const outputIndex = event.output_index;
    const key = `${outputIndex}:${event.content_index ?? 0}`;
    if (
      event.type === "response.output_item.added" &&
      event.item.type === "function_call"
    ) {
      const block = {
        index: nextIndex++,
        outputIndex,
        open: true,
        arguments: false,
      };
      blocks.set(key, block);
      tools = true;
      yield {
        type: "content_block_start",
        index: block.index,
        content_block: {
          type: "tool_use",
          id: event.item.call_id,
          name: event.item.name,
          input: {},
        },
      };
    } else if (event.type === "response.output_text.delta") {
      let block = blocks.get(key);
      if (!block) {
        block = { index: nextIndex++, outputIndex, open: true };
        blocks.set(key, block);
        yield {
          type: "content_block_start",
          index: block.index,
          content_block: { type: "text", text: "" },
        };
      }
      yield {
        type: "content_block_delta",
        index: block.index,
        delta: { type: "text_delta", text: event.delta },
      };
    } else if (event.type === "response.function_call_arguments.delta") {
      const block = blocks.get(key);
      if (!block?.open)
        throw new Error("Copilot sent arguments without a tool call.");
      block.arguments = true;
      yield {
        type: "content_block_delta",
        index: block.index,
        delta: { type: "input_json_delta", partial_json: event.delta },
      };
    } else if (event.type === "response.output_item.done") {
      for (const block of blocks.values()) {
        if (!block.open || block.outputIndex !== outputIndex) continue;
        if (event.item.type === "function_call" && !block.arguments) {
          yield {
            type: "content_block_delta",
            index: block.index,
            delta: {
              type: "input_json_delta",
              partial_json: event.item.arguments,
            },
          };
        }
        block.open = false;
        yield { type: "content_block_stop", index: block.index };
      }
    } else if (
      event.type === "response.completed" ||
      event.type === "response.incomplete"
    ) {
      for (const block of blocks.values()) {
        if (block.open)
          yield { type: "content_block_stop", index: block.index };
      }
      yield {
        type: "message_delta",
        delta: {
          stop_reason: stopReason(event.response, tools),
          stop_sequence: null,
        },
        usage: usage(event.response.usage),
      };
      yield { type: "message_stop" };
      return;
    } else if (event.type === "error" || event.type === "response.failed") {
      throw new Error(
        event.message ??
          event.response?.error?.message ??
          "Copilot response failed.",
      );
    }
  }
  throw new Error("Copilot ended the stream without a completed response.");
}

export async function startGateway(credential, effort) {
  const key = randomBytes(32).toString("hex");
  const server = createServer(async (request, response) => {
    const supplied =
      request.headers.authorization?.replace(/^Bearer /, "") ??
      request.headers["x-api-key"] ??
      "";
    const left = Buffer.from(supplied);
    const right = Buffer.from(key);
    if (left.length !== right.length || !timingSafeEqual(left, right)) {
      response.writeHead(401).end(
        JSON.stringify({
          type: "error",
          error: {
            type: "authentication_error",
            message: "Invalid gateway credential.",
          },
        }),
      );
      return;
    }
    if (
      request.method !== "POST" ||
      request.url?.split("?")[0] !== "/v1/messages"
    ) {
      response.writeHead(404).end();
      return;
    }
    const cancel = new AbortController();
    response.on("close", () => {
      if (!response.writableEnded) cancel.abort();
    });
    try {
      let bytes = "";
      request.setEncoding("utf8");
      let size = 0;
      for await (const chunk of request) {
        size += Buffer.byteLength(chunk);
        if (size > 64 * 1024 * 1024) {
          response.writeHead(413).end();
          return;
        }
        bytes += chunk;
      }
      const body = JSON.parse(bytes);
      const payload = {
        ...toResponses(body, effort),
        stream: Boolean(body.stream),
      };
      const upstream = await fetch(`${API}/responses`, {
        method: "POST",
        headers: headers(credential),
        body: JSON.stringify(payload),
        redirect: "error",
        signal: AbortSignal.any([
          cancel.signal,
          AbortSignal.timeout(10 * 60_000),
        ]),
      });
      if (!upstream.ok) {
        response
          .writeHead(upstream.status, { "Content-Type": "application/json" })
          .end(
            JSON.stringify({
              type: "error",
              error: {
                type: "api_error",
                message: await responseError(upstream, credential),
              },
            }),
          );
      } else if (!body.stream) {
        response
          .writeHead(200, { "Content-Type": "application/json" })
          .end(JSON.stringify(toMessage(await upstream.json(), body.model)));
      } else {
        response.writeHead(200, {
          "Content-Type": "text/event-stream",
          "Cache-Control": "no-cache",
        });
        for await (const event of toAnthropicEvents(
          readEvents(upstream.body),
          body.model,
        )) {
          if (
            !response.write(
              `event: ${event.type}\ndata: ${JSON.stringify(event)}\n\n`,
            )
          )
            await once(response, "drain", { signal: cancel.signal });
        }
        response.end();
      }
    } catch (error) {
      if (cancel.signal.aborted) return;
      const value = {
        type: "error",
        error: {
          type: "api_error",
          message: redact(error.message, credential, key),
        },
      };
      if (response.headersSent)
        response.end(`event: error\ndata: ${JSON.stringify(value)}\n\n`);
      else
        response
          .writeHead(400, { "Content-Type": "application/json" })
          .end(JSON.stringify(value));
    }
  });
  server.listen(0, "127.0.0.1");
  await once(server, "listening");
  return {
    url: `http://127.0.0.1:${server.address().port}`,
    key,
    close: () => {
      server.closeAllConnections();
      server.close();
    },
  };
}

export function prepareLaunch(rawArgs, models) {
  const args = [...rawArgs];
  const selected = args[args.indexOf("--model") + 1];
  modelId(selected);
  const index = args.indexOf("--settings");
  let settings;
  try {
    settings = index < 0 ? {} : JSON.parse(args[index + 1]);
    if (!settings || typeof settings !== "object" || Array.isArray(settings))
      throw new Error();
  } catch {
    throw new Error("Claude session settings must be a JSON object.");
  }
  const model = models.find((entry) => entry.id === modelId(selected));
  if (!model)
    throw new Error(
      `Copilot does not expose the 1M context variant for ${selected}.`,
    );
  const levels = model.supportedEffortLevels ?? [];
  const effortIndex = args.indexOf("--effort");
  const effort =
    effortIndex >= 0
      ? args[effortIndex + 1]
      : levels.includes("medium")
        ? "medium"
        : levels[0];
  if (effort && !levels.includes(effort)) {
    throw new Error(
      `Copilot does not support effort ${effort} for ${selected}.`,
    );
  }
  // Claude replaces Copilot's None effort. The gateway preserves that value.
  if (effortIndex >= 0) args.splice(effortIndex, 2);
  if (effort === "none") settings.alwaysThinkingEnabled = false;
  else if (effort) args.push("--effort", effort);
  return { args, settings, model, selected, effort };
}

async function main() {
  const [command, ...rawArgs] = process.argv.slice(2);
  if (command === "--list-models") {
    process.stdout.write(JSON.stringify(await listModels()));
    return;
  }
  if (!command) throw new Error("A Claude Code executable is required.");
  const credential = token();
  const models = await listModels(credential);
  const { args, settings, model, selected, effort } = prepareLaunch(
    rawArgs,
    models,
  );
  const gateway = await startGateway(credential, effort);
  settings.env = {
    ...settings.env,
    ANTHROPIC_BASE_URL: gateway.url,
    ANTHROPIC_AUTH_TOKEN: gateway.key,
    ANTHROPIC_API_KEY: "",
    ENABLE_TOOL_SEARCH: "false",
    CLAUDE_CODE_USE_BEDROCK: "0",
    CLAUDE_CODE_USE_VERTEX: "0",
    CLAUDE_CODE_USE_FOUNDRY: "0",
    CLAUDE_CODE_MAX_CONTEXT_TOKENS: String(model.contextWindow),
    ANTHROPIC_DEFAULT_OPUS_MODEL: selected,
    ANTHROPIC_DEFAULT_SONNET_MODEL: selected,
    ANTHROPIC_DEFAULT_HAIKU_MODEL: selected,
    ANTHROPIC_DEFAULT_FABLE_MODEL: selected,
  };
  settings.modelPicker = {
    replaceBuiltInOptions: true,
    options: models.map((entry) => ({
      model: `copilot/${entry.id}`,
      label: entry.name,
    })),
  };
  const index = args.indexOf("--settings");
  if (index < 0) args.push("--settings", JSON.stringify(settings));
  else args[index + 1] = JSON.stringify(settings);
  const child = spawn(command, args, { stdio: "inherit", windowsHide: true });
  for (const signal of ["SIGINT", "SIGTERM"])
    process.on(signal, () => child.kill(signal));
  child.on("error", (error) => {
    console.error(error.message);
    gateway.close();
    process.exitCode = 1;
  });
  child.on("exit", (code) => {
    gateway.close();
    process.exitCode = code ?? 1;
  });
}

if (
  process.argv[1] &&
  import.meta.url === pathToFileURL(process.argv[1]).href
) {
  main().catch((error) => {
    console.error(error.message);
    process.exitCode = 1;
  });
}
