import { describe, expect, it, vi } from "vitest";
import { createReleaseNotesLoader } from "./releaseNotesLoader";

const changelog = "## [0.1.2]\n\nNew release.\n\n## [0.1.1]\n\nOlder release.";

describe("optional release note bodies", () => {
  it("reads bundled notes offline without requesting GitHub", async () => {
    const published = vi.fn().mockRejectedValue(new Error("offline"));
    const loader = createReleaseNotesLoader(async () => changelog, published);
    expect((await loader.load("0.1.2"))?.markdown).toContain("New release.");
    expect(published).not.toHaveBeenCalled();
  });

  it("shares requests by version and keeps out-of-order published responses separate", async () => {
    const finish = new Map<string, (body: string) => void>();
    const published = vi.fn(
      (version: string) =>
        new Promise<string>((resolve) => {
          finish.set(version, resolve);
        }),
    );
    const loader = createReleaseNotesLoader(async () => "", published);
    const older = loader.load("0.13.10");
    const same = loader.load("0.13.10");
    const newer = loader.load("0.13.11");
    await Promise.resolve();
    await Promise.resolve();
    expect(published).toHaveBeenCalledTimes(2);
    expect(loader.get("0.13.11")).toBeUndefined();
    finish.get("0.13.11")!("## [0.13.11]\n\nNewer.");
    expect((await newer)?.source.version).toBe("0.13.11");
    finish.get("0.13.10")!("## [0.13.10]\n\nOlder.");
    expect(await same).toEqual(await older);
    expect(loader.get("0.13.10")?.markdown).toContain("Older.");
    expect(loader.get("0.13.11")?.markdown).toContain("Newer.");
    await loader.load("0.13.10");
    expect(published).toHaveBeenCalledTimes(2);
  });

  it("retries failed and not-yet-published versions without caching their absence", async () => {
    const read = vi.fn(async () => "");
    const published = vi
      .fn()
      .mockRejectedValueOnce(new Error("offline"))
      .mockResolvedValueOnce(null)
      .mockResolvedValue("## [0.13.11]\n\nAvailable now.");
    const loader = createReleaseNotesLoader(read, published);
    await expect(loader.load("0.13.11")).rejects.toThrow("offline");
    expect(await loader.load("0.13.11")).toBeNull();
    expect((await loader.load("0.13.11"))?.markdown).toContain(
      "Available now.",
    );
    expect(read).toHaveBeenCalledOnce();
    expect(published).toHaveBeenCalledTimes(3);
  });

  it("does no eager read and shares one import across different versions", async () => {
    let finish!: (value: string) => void;
    const read = vi.fn(
      () =>
        new Promise<string>((resolve) => {
          finish = resolve;
        }),
    );
    const loader = createReleaseNotesLoader(read);
    expect(loader.get("0.1.1")).toBeUndefined();
    expect(read).not.toHaveBeenCalled();
    const newer = loader.load("0.1.2");
    const older = loader.load("0.1.1");
    expect(read).toHaveBeenCalledOnce();
    finish(changelog);
    expect((await newer)?.markdown).toContain("New release.");
    expect((await older)?.markdown).toContain("Older release.");
    expect(loader.get("0.1.1")?.source.version).toBe("0.1.1");
    expect(await loader.load("missing")).toBeNull();
    expect(read).toHaveBeenCalledOnce();
  });

  it("retries an import failure instead of caching the rejection", async () => {
    const read = vi
      .fn()
      .mockRejectedValueOnce(new Error("offline"))
      .mockResolvedValue(changelog);
    const loader = createReleaseNotesLoader(read);
    await expect(loader.load("0.1.1")).rejects.toThrow("offline");
    expect(loader.get("0.1.1")).toBeUndefined();
    expect((await loader.load("0.1.1"))?.markdown).toContain("Older release.");
    expect(read).toHaveBeenCalledTimes(2);
  });
});
