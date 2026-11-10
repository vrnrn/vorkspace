/** Own the deadline and cancellation from submission through the final frame. */
export async function settleTextExchange(
  start,
  completed,
  { label, timeoutMs, signal },
) {
  let timer;
  let abort;
  const deadline = new Promise((_, reject) => {
    timer = setTimeout(
      () => reject(new Error(`${label} text generation timed out`)),
      timeoutMs,
    );
    if (signal) {
      abort = () => reject(new Error("Text request cancelled"));
      signal.addEventListener("abort", abort, { once: true });
      if (signal.aborted) abort();
    }
  });
  try {
    const submitted = Promise.resolve().then(() => {
      signal?.throwIfAborted();
      return start();
    });
    // A child may deliver its final frame or exit before acknowledging input.
    await Promise.race([Promise.all([submitted, completed]), deadline]);
  } finally {
    clearTimeout(timer);
    if (abort) signal.removeEventListener("abort", abort);
  }
}
