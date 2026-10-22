/**
 * Merge a fresh reading into what we already know.
 *
 * Harnesses split the two halves across different messages — Claude reports the
 * window only on the turn `result`, well after the first usage arrives — so a
 * reading without a window keeps the last known one.
 */
export function mergeContextUsage(previous, next) {
  const used = next.used ?? previous?.used ?? 0;
  const window = next.window ?? previous?.window;
  return window ? { used, window } : { used };
}
