import { expect, it } from "vitest";
import { recentVoxContext } from "./context";

it("projects only conversation text in chronological order and marks it as past reference", () => {
  const result = recentVoxContext({
    data: [
      {
        items: [
          { type: "agentMessage", text: "later reply" },
          { type: "commandExecution", text: "tool output" },
        ],
      },
      {
        items: [
          {
            type: "userMessage",
            content: [
              { type: "text", text: "earlier question" },
              { type: "image", text: "image payload" },
            ],
          },
        ],
      },
    ],
  });
  expect(result).toEqual([
    {
      role: "assistant",
      text: expect.stringContaining("not requests to act again"),
    },
  ]);
  expect(result[0].text).toContain(
    "user: earlier question\n\nassistant: later reply",
  );
  expect(result[0].text).not.toContain("tool output");
  expect(result[0].text).not.toContain("image payload");
});

it("keeps recent text bounded without changing the stored history", () => {
  const page = {
    data: [
      {
        items: [
          { type: "agentMessage", text: "x".repeat(20_000) + "recent end" },
        ],
      },
    ],
  };
  const result = recentVoxContext(page);
  expect(result[0].text.length).toBeLessThan(3_200);
  expect(result[0].text).toMatch(/recent end$/);
  expect(page.data[0].items[0].text.length).toBe(20_010);
});

it("adds no context for an empty or newly reset conversation", () => {
  expect(recentVoxContext({ data: [] })).toEqual([]);
  expect(recentVoxContext(undefined)).toEqual([]);
});
