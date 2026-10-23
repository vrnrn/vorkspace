import { randomUUID } from "node:crypto";
// The desktop rejects host responses over 16 MiB. Syncs above the inline limit
// are served as pieces of one serialized revision, each well under that cap
// after JSON string escaping, so neither a long transcript nor a single huge
// block can produce an oversized response.
export const INLINE_SYNC_BYTES = 4 * 1024 * 1024;
export const SYNC_CHUNK_BYTES = 4 * 1024 * 1024;
const TRANSFER_TTL_MS = 2 * 60_000;
const MAX_TRANSFERS = 8;
export const MAX_TRANSFER_BYTES = 64 * 1024 * 1024;
export const MAX_RETAINED_TRANSFER_BYTES = 128 * 1024 * 1024;
const encodedBytes = (value) => Buffer.byteLength(JSON.stringify(value));
export class SyncTransfers {
  limits;
  transfers = new Map();
  expiryTimer;
  retainedBytes = 0;
  constructor(
    limits = {
      inline: INLINE_SYNC_BYTES,
      chunk: SYNC_CHUNK_BYTES,
    },
  ) {
    this.limits = {
      inline: INLINE_SYNC_BYTES, chunk: SYNC_CHUNK_BYTES,
      transfer: MAX_TRANSFER_BYTES, retained: MAX_RETAINED_TRANSFER_BYTES,
      count: MAX_TRANSFERS, ...limits,
    };
    for (const value of Object.values(this.limits))
      if (!Number.isSafeInteger(value) || value <= 0)
        throw new Error("Invalid session transfer limit");
    if (this.limits.chunk < 8) throw new Error("Session chunk limit is too small");
  }
  respond(sessionId, sync) {
    if (sync.kind === "unchanged") return sync;
    this.prune();
    // Count exact JSON wire bytes and UTF-16 units before constructing its text.
    // The scanner stops at the single-transfer cap, so a huge provider field
    // cannot first allocate an arbitrarily large escaped serialization.
    const size = jsonSize(sync, this.limits.transfer);
    if (size.bytes <= this.limits.inline) return sync;
    const retained = size.units * 2; // conservative JS string backing storage
    if (this.transfers.size >= this.limits.count || retained > this.limits.retained - this.retainedBytes)
      throw new Error("Session transfer capacity is busy; retry after active transfers complete");
    const text = JSON.stringify(sync);
    const transfer = randomUUID();
    this.transfers.set(transfer, {
      sessionId,
      text,
      retained,
      expires: Date.now() + TRANSFER_TTL_MS,
    });
    this.retainedBytes += retained;
    this.scheduleExpiry();
    return { kind: "chunked", transfer, length: text.length };
  }
  chunk(sessionId, transfer, offset) {
    const entry = this.transfers.get(transfer);
    if (!entry || entry.sessionId !== sessionId || entry.expires <= Date.now())
      throw new Error("Session transfer expired; reload the session");
    if (
      !Number.isSafeInteger(offset) ||
      offset < 0 ||
      offset > entry.text.length
    )
      throw new Error("Invalid session transfer offset");
    // Transcript JSON usually encodes near one byte per UTF-16 unit; shrink
    // the piece when escaping or non-ASCII text makes it larger.
    let end = Math.min(
      entry.text.length,
      offset + Math.max(1, Math.floor((this.limits.chunk * 2) / 3)),
    );
    while (
      end - offset > 1 &&
      encodedBytes(entry.text.slice(offset, end)) > this.limits.chunk
    )
      end = offset + Math.ceil((end - offset) / 2);
    // Never split a surrogate pair: a lone surrogate is not valid JSON text
    // for the desktop's native parser.
    const code = entry.text.charCodeAt(end - 1);
    if (
      end < entry.text.length &&
      end - offset > 1 &&
      code >= 0xd800 &&
      code <= 0xdbff
    )
      end--;
    const data = entry.text.slice(offset, end);
    entry.expires = Date.now() + TRANSFER_TTL_MS;
    if (end === entry.text.length) this.remove(transfer);
    this.scheduleExpiry();
    return { data };
  }
  prune() {
    const now = Date.now();
    for (const [id, entry] of this.transfers)
      if (entry.expires <= now) this.remove(id);
  }
  remove(id) {
    const entry = this.transfers.get(id);
    if (entry) this.retainedBytes -= entry.retained;
    this.transfers.delete(id);
  }
  scheduleExpiry() {
    clearTimeout(this.expiryTimer);
    this.expiryTimer = undefined;
    if (!this.transfers.size) return;
    const next = Math.min(
      ...[...this.transfers.values()].map((entry) => entry.expires),
    );
    // Abandoned downloads must release their serialized transcript even when
    // no other request arrives. One timer covers all (at most eight) transfers.
    this.expiryTimer = setTimeout(() => {
      this.prune();
      this.scheduleExpiry();
    }, Math.max(0, next - Date.now()));
    this.expiryTimer.unref?.();
  }
  close() {
    clearTimeout(this.expiryTimer);
    this.expiryTimer = undefined;
    this.transfers.clear();
    this.retainedBytes = 0;
  }
}

// Mirrors JSON encoding for the plain data produced by HostStore.sync. It does
// not allocate escaped strings. A cap is enforced throughout each string scan,
// rather than after traversing an oversized value. Active ancestors detect
// cycles while allowing the same object in separate branches.
export function jsonSize(value, limit) {
  let bytes = 0;
  let units = 0;
  const ancestors = new Set();
  function add(wire, codeUnits = wire) {
    bytes += wire;
    units += codeUnits;
    if (bytes > limit)
      throw new Error(`Session transfer exceeds the ${limit}-byte limit; reduce retained session output before reloading`);
  }
  function string(text) {
    add(2);
    for (let index = 0; index < text.length; index++) {
      const code = text.charCodeAt(index);
      if (code === 34 || code === 92 || code === 8 || code === 9 || code === 10 || code === 12 || code === 13) add(2);
      else if (code < 32) add(6);
      else if (code < 128) add(1);
      else if (code < 2048) add(2, 1);
      else if (code >= 0xd800 && code <= 0xdbff && text.charCodeAt(index + 1) >= 0xdc00 && text.charCodeAt(index + 1) <= 0xdfff) {
        add(4, 2);
        index++;
      } else if (code >= 0xd800 && code <= 0xdfff) add(6);
      else add(3, 1);
    }
  }
  function visit(item) {
    if (typeof item === "string") return string(item);
    if (item === null || typeof item === "number" || typeof item === "boolean") return add(JSON.stringify(item).length);
    if (typeof item !== "object") throw new Error("Session transfer contains unsupported JSON data");
    if (ancestors.has(item)) throw new Error("Session transfer contains circular JSON data");
    // Stored snapshots are plain JSON. Reject custom serialization behavior so
    // a toJSON hook cannot bypass the count between measurement and encoding.
    if (typeof item.toJSON === "function") throw new Error("Session transfer contains custom JSON data");
    ancestors.add(item);
    add(2);
    let count = 0;
    if (Array.isArray(item)) {
      for (const child of item) {
        if (count++) add(1);
        if (child === undefined || typeof child === "function" || typeof child === "symbol") add(4);
        else visit(child);
      }
    } else {
      for (const key of Object.keys(item)) {
        const child = item[key];
        if (child === undefined || typeof child === "function" || typeof child === "symbol") continue;
        if (count++) add(1);
        string(key);
        add(1);
        visit(child);
      }
    }
    ancestors.delete(item);
  }
  visit(value);
  return { bytes, units };
}
