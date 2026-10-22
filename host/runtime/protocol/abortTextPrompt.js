export function btwTextCancelled() {
  return new Error("By-the-way request cancelled");
}
/** Races an isolated text prompt against an AbortSignal without stopping the backend globally. */
export function abortTextPromptRace(signal, onAbort) {
  if (!signal) return { promise: null, detach: () => undefined };
  let handler;
  const promise = new Promise((_, reject) => {
    handler = () => {
      void Promise.resolve(onAbort()).finally(() => reject(btwTextCancelled()));
    };
    signal.addEventListener("abort", handler, { once: true });
    if (signal.aborted) handler();
  });
  return {
    promise,
    detach: () => {
      if (handler) signal.removeEventListener("abort", handler);
    },
  };
}
