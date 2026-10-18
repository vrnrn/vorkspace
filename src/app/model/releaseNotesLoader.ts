import { releaseNotesForVersion } from "./releaseNotes";

/** Share the optional body import; failures must allow a later open to retry. */
export function createReleaseNotesLoader(read: () => Promise<string>) {
  let content: string | undefined;
  let pending: Promise<string> | undefined;
  return {
    get(version: string) {
      return content === undefined
        ? undefined
        : releaseNotesForVersion(version, content);
    },
    async load(version: string) {
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
      return releaseNotesForVersion(version, await pending);
    },
  };
}

const changelogModules = import.meta.glob<string>("../../../CHANGELOG.md", {
  query: "?raw",
  import: "default",
});

export const releaseNotesLoader = createReleaseNotesLoader(async () => {
  const load = changelogModules["../../../CHANGELOG.md"];
  return load ? await load() : "";
});
