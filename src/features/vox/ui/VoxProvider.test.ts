// @vitest-environment happy-dom
import { act, createElement } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { saveVoxEnabled } from "../model/availability";
import { VOX_CONVERSATION_KEY } from "../model/conversation";
import { stopVox } from "../model/lifecycle";
import { useVox, VoxProvider, type VoxControls } from "./VoxProvider";

type Options = ConstructorParameters<typeof import("../model/vox").Vox>[0];
const transport = vi.hoisted(() => ({
  created: vi.fn(),
  start: vi.fn(async () => {}),
  stop: vi.fn(async () => {}),
}));
vi.mock("../model/vox", () => ({
  Vox: class {
    constructor(readonly options: Options) {
      transport.created(this);
    }
    async start() {
      await transport.start();
      this.options.onConnected();
    }
    stop() {
      return transport.stop();
    }
    setSilent() {}
  },
}));

function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (reason: unknown) => void;
  const promise = new Promise<T>((yes, no) => {
    resolve = yes;
    reject = no;
  });
  return { promise, resolve, reject };
}

let root: Root;
let container: HTMLDivElement;
let controls: VoxControls;
function Probe() {
  controls = useVox()!;
  return null;
}
async function render(cwd: string | (() => Promise<string>) = "/repo") {
  await act(async () =>
    root.render(
      createElement(VoxProvider, {
        cwd,
        execute: vi.fn(),
        children: createElement(Probe),
      }),
    ),
  );
}
beforeEach(() => {
  vi.stubGlobal("IS_REACT_ACT_ENVIRONMENT", true);
  const saved = new Map<string, string>();
  vi.stubGlobal("localStorage", {
    getItem: (key: string) => saved.get(key) ?? null,
    setItem: (key: string, value: string) => saved.set(key, value),
  });
  transport.created.mockClear();
  transport.start.mockReset().mockResolvedValue(undefined);
  transport.stop.mockReset().mockResolvedValue(undefined);
  saveVoxEnabled(true);
  container = document.createElement("div");
  document.body.append(container);
  root = createRoot(container);
});
afterEach(async () => {
  await act(async () => root.unmount());
  container.remove();
  vi.unstubAllGlobals();
});

it("close cancels a start waiting for its directory before any transport is created", async () => {
  const directory = deferred<string>();
  await render(() => directory.promise);
  await act(async () => controls.start());
  expect(controls.state).toBe("connecting");
  await act(async () => stopVox());
  expect(controls.state).toBe("idle");
  await act(async () => directory.resolve("/repo"));
  expect(transport.created).not.toHaveBeenCalled();
  expect(transport.start).not.toHaveBeenCalled();
  // Cancellation does not prevent a later deliberate call.
  await act(async () => controls.start());
  expect(controls.state).toBe("live");
  expect(transport.start).toHaveBeenCalledOnce();
});

it("close cancels startup before the lazy import has completed", async () => {
  await render();
  await act(async () => {
    controls.start();
    await stopVox();
  });
  expect(controls.state).toBe("idle");
  expect(transport.created).not.toHaveBeenCalled();
});

it("close cancels a successor waiting for the previous call to flush", async () => {
  await render();
  await act(async () => controls.start());
  const flush = deferred<void>();
  transport.stop.mockReturnValueOnce(flush.promise);
  let closing!: Promise<void>;
  await act(async () => {
    controls.stop();
    controls.start();
    closing = stopVox();
  });
  expect(controls.state).toBe("idle");
  await act(async () => {
    flush.resolve();
    await closing;
  });
  expect(transport.created).toHaveBeenCalledOnce();
});

it("every concurrent End observes a failed save and the failure remains visible", async () => {
  await render();
  await act(async () => controls.start());
  const flush = deferred<void>();
  transport.stop.mockReturnValueOnce(flush.promise);
  let results!: Promise<PromiseSettledResult<void>[]>;
  await act(async () => {
    results = Promise.allSettled([controls.end(), controls.end()]);
    flush.reject(new Error("latest speech was not saved"));
    await results;
  });
  expect(await results).toEqual([
    { status: "rejected", reason: expect.any(Error) },
    { status: "rejected", reason: expect.any(Error) },
  ]);
  expect(controls.error).toBe("latest speech was not saved");
  expect(transport.stop).toHaveBeenCalledOnce();
  // The completed cleanup is not a permanent barrier to a later close.
  await act(async () => controls.end());
  expect(transport.stop).toHaveBeenCalledOnce();
});

it("cross-window reset handles a failed flush and shares the failure with an overlapping close", async () => {
  await render();
  await act(async () => controls.start());
  const flush = deferred<void>();
  transport.stop.mockReturnValueOnce(flush.promise);
  let closing!: Promise<PromiseSettledResult<void>[]>;
  await act(async () =>
    window.dispatchEvent(
      new StorageEvent("storage", {
        key: VOX_CONVERSATION_KEY,
        oldValue: JSON.stringify({ revision: "old" }),
        newValue: JSON.stringify({ revision: "new" }),
      }),
    ),
  );
  await act(async () => {
    closing = Promise.allSettled([stopVox()]);
    flush.reject(new Error("latest speech was not saved"));
    await closing;
  });
  expect(controls.state).toBe("idle");
  expect(controls.error).toBe("latest speech was not saved");
  expect(await closing).toEqual([
    { status: "rejected", reason: expect.any(Error) },
  ]);
});
