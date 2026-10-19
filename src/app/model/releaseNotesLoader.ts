import { invoke } from "@tauri-apps/api/core";
import { releaseNotesForVersion } from "./releaseNotes";
import type { ReleaseNotesDocument } from "./releaseNotes";

/** Bundled notes work offline; other versions are read only when opened. */
export function createReleaseNotesLoader(
  read: () => Promise<string>,
  readPublished?: (version: string) => Promise<string | null>,
) {
  let content: string | undefined;
  let pending: Promise<string> | undefined;
  const documents = new Map<string, ReleaseNotesDocument>();
  const requests = new Map<string, Promise<ReleaseNotesDocument | null>>();
  return {
    get(version: string) {
      const cached = documents.get(version);
      if (cached) return cached;
      if (content === undefined) return undefined;
      return (
        releaseNotesForVersion(version, content) ??
        (readPublished ? undefined : null)
      );
    },
    load(version: string): Promise<ReleaseNotesDocument | null> {
      const cached = documents.get(version);
      if (cached) return Promise.resolve(cached);
      const existing = requests.get(version);
      if (existing) return existing;
      pending ??= read().then(
        (value) => {
          content = value;
          return value;
        },
        (error: unknown) => {
          pending = undefined;
          throw error;
        },
      );
      const request = pending
        .then(async (body) => {
          const bundled = releaseNotesForVersion(version, body);
          if (bundled || !readPublished) return bundled;
          const published = await readPublished(version);
          return published ? releaseNotesForVersion(version, published) : null;
        })
        .then((document) => {
          if (document) documents.set(version, document);
          return document;
        })
        .finally(() => requests.delete(version));
      requests.set(version, request);
      return request;
    },
  };
}

const bundledModules = import.meta.glob<string>(
  "../../../build/release-notes.md",
  {
    query: "?raw",
    import: "default",
  },
);

export const releaseNotesLoader = createReleaseNotesLoader(
  async () => {
    const load = bundledModules["../../../build/release-notes.md"];
    return load ? await load() : "";
  },
  (version) => invoke<string | null>("read_release_notes", { version }),
);
