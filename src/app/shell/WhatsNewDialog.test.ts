import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it, vi } from "vitest";
import { releaseNotesForVersion } from "../model/releaseNotes";
import { WhatsNewBody } from "./WhatsNewDialog";

const FIXTURE = `## [0.1.25] - 2026-03-10

- Added faster workspace startup and incremental session saves.
`;

vi.mock("../model/releaseNotesLoader", () => ({
  releaseNotesLoader: {
    get: (version: string) => releaseNotesForVersion(version, FIXTURE),
    load: async (version: string) => releaseNotesForVersion(version, FIXTURE),
  },
}));

describe("WhatsNewBody", () => {
  it("renders the version notes without the changelog heading", async () => {
    const markup = renderToStaticMarkup(
      createElement(WhatsNewBody, { version: "0.1.25" }),
    );

    expect(markup).toContain("whats-new-md");
    expect(markup).toContain("What&#x27;s new in vorkspace 0.1.25");
    expect(markup).not.toContain("## [0.1.25]");
  });
});
