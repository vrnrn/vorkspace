// The provider owns both pending setup and the live call. App close must use
// that owner, rather than looking only for an instantiated transport.
let close: (() => Promise<void>) | undefined;

export function registerVoxCloser(closer: () => Promise<void>) {
  close = closer;
  return () => {
    if (close === closer) close = undefined;
  };
}

export async function stopVox(): Promise<void> {
  await close?.();
}
