import { afterEach, expect, it, vi } from "vitest";
import { SyncTransfers, jsonSize } from "./sync-transfer.js";
afterEach(() => vi.useRealTimers());
const sync = (text) => ({
  kind: "delta",
  base: 1,
  value: {
    projectId: "project",
    revision: 2,
    status: "idle",
    updatedAt: 0,
    session: {
      id: "session",
      harness: "codex",
      model: "codex:test",
      runtimeMode: "supervised",
      cwd: "/repo",
      title: "Session",
    },
  },
  blockIds: ["block"],
  blocks: [{ id: "block", role: "assistant", text }],
});
function read(transfers, transfer, length) {
  const pieces = [];
  for (let offset = 0; offset < length;) {
    const { data } = transfers.chunk("session", transfer, offset);
    pieces.push(data);
    offset += data.length;
  }
  return pieces;
}
it("returns small syncs inline", () => {
  const transfers = new SyncTransfers({ inline: 1024, chunk: 256 });
  expect(transfers.respond("session", sync("short"))).toEqual(sync("short"));
});
it("splits large syncs into bounded pieces without splitting characters", () => {
  const transfers = new SyncTransfers({ inline: 1024, chunk: 256 });
  const value = sync('🚀"\u0001'.repeat(2_000));
  const response = transfers.respond("session", value);
  if (response.kind !== "chunked") throw new Error("Expected a transfer");
  const pieces = read(transfers, response.transfer, response.length);
  expect(pieces.length).toBeGreaterThan(10);
  for (const piece of pieces) {
    expect(Buffer.byteLength(JSON.stringify(piece))).toBeLessThanOrEqual(256);
    expect(piece).not.toMatch(
      /[\uD800-\uDBFF](?![\uDC00-\uDFFF])|(?<![\uD800-\uDBFF])[\uDC00-\uDFFF]/,
    );
  }
  expect(JSON.parse(pieces.join(""))).toEqual(value);
});
it("serves a transfer only for its session and forgets it after the last piece", () => {
  const transfers = new SyncTransfers({ inline: 64, chunk: 4096 });
  const response = transfers.respond("session", sync("x".repeat(500)));
  if (response.kind !== "chunked") throw new Error("Expected a transfer");
  expect(() => transfers.chunk("other", response.transfer, 0)).toThrow(
    "expired",
  );
  expect(() => transfers.chunk("session", response.transfer, -1)).toThrow(
    "offset",
  );
  read(transfers, response.transfer, response.length);
  expect(() => transfers.chunk("session", response.transfer, 0)).toThrow(
    "expired",
  );
});
it("expires an abandoned transfer without waiting for another host request", () => {
  vi.useFakeTimers();
  const transfers = new SyncTransfers({ inline: 64, chunk: 256 });
  transfers.respond("session", sync("x".repeat(500)));
  expect(transfers.transfers.size).toBe(1);
  expect(vi.getTimerCount()).toBe(1);
  vi.advanceTimersByTime(120_000);
  expect(transfers.transfers.size).toBe(0);
  expect(vi.getTimerCount()).toBe(0);
});
it("extends only an active transfer's expiry while retaining one cleanup timer", () => {
  vi.useFakeTimers();
  const transfers = new SyncTransfers({ inline: 64, chunk: 256 });
  const active = transfers.respond("session", sync("x".repeat(500)));
  const abandoned = transfers.respond("session", sync("y".repeat(500)));
  vi.advanceTimersByTime(119_999);
  transfers.chunk("session", active.transfer, 0);
  expect(vi.getTimerCount()).toBe(1);
  vi.advanceTimersByTime(1);
  expect(transfers.transfers.has(abandoned.transfer)).toBe(false);
  expect(transfers.transfers.has(active.transfer)).toBe(true);
  vi.advanceTimersByTime(119_999);
  expect(transfers.transfers.size).toBe(0);
  expect(vi.getTimerCount()).toBe(0);
});
it("clears retained data and cleanup work on completion or server shutdown", () => {
  vi.useFakeTimers();
  const transfers = new SyncTransfers({ inline: 64, chunk: 4096 });
  const complete = transfers.respond("session", sync("x".repeat(500)));
  read(transfers, complete.transfer, complete.length);
  expect(vi.getTimerCount()).toBe(0);
  for (let index = 0; index < 8; index++)
    transfers.respond("session", sync("y".repeat(500)));
  expect(transfers.transfers.size).toBe(8);
  expect(() => transfers.respond("session", sync("capacity"))).toThrow("busy");
  expect(vi.getTimerCount()).toBe(1);
  transfers.close();
  expect(transfers.transfers.size).toBe(0);
  expect(vi.getTimerCount()).toBe(0);
});

it("rejects excess admissions without evicting active transfers or serializing the rejected value", () => {
  const value = sync("x".repeat(500));
  const length = JSON.stringify(value).length;
  const transfers = new SyncTransfers({ inline: 64, chunk: 4096, retained: length * 4, count: 8 });
  const first = transfers.respond("session", value);
  const second = transfers.respond("session", value);
  expect(transfers.retainedBytes).toBe(length * 4);
  const stringify = vi.spyOn(JSON, "stringify");
  expect(() => transfers.respond("session", value)).toThrow("capacity is busy; retry");
  // Numeric scalar measurement may stringify; no object or large text encoding.
  expect(stringify.mock.calls.every(([value]) => typeof value === "number")).toBe(true);
  stringify.mockRestore();
  expect(transfers.transfers.has(first.transfer)).toBe(true);
  expect(transfers.transfers.has(second.transfer)).toBe(true);
  read(transfers, first.transfer, first.length);
  expect(transfers.retainedBytes).toBe(length * 2);
  const third = transfers.respond("session", value);
  expect(third.kind).toBe("chunked");
  transfers.close();
  expect(transfers.retainedBytes).toBe(0);
});

it("checks exact wire and escaped string limits before constructing transfer text", () => {
  const value = sync('🚀"\u0001\ud800'.repeat(100));
  const text = JSON.stringify(value);
  const bytes = Buffer.byteLength(text);
  const transfers = new SyncTransfers({ inline: 64, chunk: 256, transfer: bytes, retained: text.length * 2 });
  const admitted = transfers.respond("session", value);
  expect(transfers.retainedBytes).toBe(text.length * 2);
  expect(JSON.parse(read(transfers, admitted.transfer, admitted.length).join(""))).toEqual(value);
  expect(transfers.retainedBytes).toBe(0);
  const rejected = new SyncTransfers({ inline: 64, chunk: 256, transfer: bytes - 1 });
  const stringify = vi.spyOn(JSON, "stringify");
  expect(() => rejected.respond("session", value)).toThrow("exceeds");
  expect(stringify.mock.calls.every(([value]) => typeof value === "number")).toBe(true);
  stringify.mockRestore();
  expect(rejected.retainedBytes).toBe(0);
  expect(rejected.transfers.size).toBe(0);
  rejected.close();
});

it("releases expired byte reservations independently and permits a waiting retry", () => {
  vi.useFakeTimers();
  const value = sync("x".repeat(500));
  const size = JSON.stringify(value).length * 2;
  const transfers = new SyncTransfers({ inline: 64, chunk: 256, retained: size });
  transfers.respond("session", value);
  expect(() => transfers.respond("session", value)).toThrow("busy");
  vi.advanceTimersByTime(120_000);
  expect(transfers.retainedBytes).toBe(0);
  expect(transfers.respond("session", value).kind).toBe("chunked");
  transfers.close();
  expect(transfers.retainedBytes).toBe(0);
  expect(vi.getTimerCount()).toBe(0);
});

it("matches native JSON sizes across metadata, escaping, UTF-8 and unpaired surrogates", () => {
  const alphabet = ['a', '"', '\\', '\n', '\u0000', '\t', '\b', '\f', '\r', 'é', '漢', '🚀', '\ud800', '\udfff'];
  const samples = alphabet.flatMap((left) => alphabet.map((right) => left + right));
  for (const text of samples) {
    const value = { text, absent: undefined, bool: true, null: null, number: -2.3e100, nonfinite: Infinity, array: [undefined, false, NaN, { [text]: text }] };
    const encoded = JSON.stringify(value);
    expect(jsonSize(value, 4096)).toEqual({ bytes: Buffer.byteLength(encoded), units: encoded.length });
  }
});

it("keeps surrogate boundaries intact at the minimum eight-byte chunk limit", () => {
  const value = sync('🚀"\u0001漢'.repeat(10));
  const transfers = new SyncTransfers({ inline: 64, chunk: 8 });
  const response = transfers.respond("session", value);
  const pieces = read(transfers, response.transfer, response.length);
  for (const piece of pieces) {
    expect(Buffer.byteLength(JSON.stringify(piece))).toBeLessThanOrEqual(8);
    expect(piece).not.toMatch(/[\uD800-\uDBFF](?![\uDC00-\uDFFF])|(?<![\uD800-\uDBFF])[\uDC00-\uDFFF]/);
  }
  expect(JSON.parse(pieces.join(""))).toEqual(value);
  expect(transfers.retainedBytes).toBe(0);
});

it("leaves no byte reservation or transfer when serialization fails after admission", () => {
  const transfers = new SyncTransfers({ inline: 64, chunk: 256 });
  const value = sync("x".repeat(500));
  const native = JSON.stringify;
  const stringify = vi.spyOn(JSON, "stringify").mockImplementation((input, ...rest) => {
    if (input === value) throw new Error("encoding failed");
    return native(input, ...rest);
  });
  try {
    expect(() => transfers.respond("session", value)).toThrow("encoding failed");
    expect(transfers.retainedBytes).toBe(0);
    expect(transfers.transfers.size).toBe(0);
    expect(transfers.expiryTimer).toBeUndefined();
  } finally {
    stringify.mockRestore();
    transfers.close();
  }
});
