import { readFileSync } from "node:fs";
import { assertDraftOwnership } from "./prepare-macos-release.mjs";

assertDraftOwnership(
  {
    tag: process.env.RELEASE_TAG,
    workflow: `https://github.com/${process.env.GITHUB_REPOSITORY}/actions/runs/${process.env.GITHUB_RUN_ID}`,
  },
  JSON.parse(readFileSync(process.argv[2], "utf8")),
  process.env.GITHUB_SHA,
);
