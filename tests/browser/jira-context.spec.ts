import { expect, test } from "@playwright/test";

for (const provider of ["jira", "github", "gitlab", "gitlab-mr"]) {
  test(`${provider}: copy and attach live context to a Bot`, async ({
    page,
  }) => {
    const errors: string[] = [];
    page.on("pageerror", (error) => errors.push(error.message));
    const mergeRequest = provider === "gitlab-mr";
    await page.goto(
      `/tests/browser/jira-context.html?provider=${mergeRequest ? "gitlab&kind=pr" : provider}`,
    );
    if (mergeRequest) {
      await expect(
        page.getByRole("button", { name: "Pipeline passed", exact: true }),
      ).toBeVisible();
      await expect(page.getByText("!42", { exact: true })).toBeVisible();
    }
    await page
      .getByRole("button", { name: "Copy context", exact: true })
      .click();
    await expect(
      page.getByRole("button", { name: "Copied", exact: true }),
    ).toBeVisible();
    await page.getByRole("button", { name: "Choose chat destination" }).click();
    await expect(
      page.getByRole("group", { name: "Projects", exact: true }),
    ).toBeVisible();
    await page.getByRole("option", { name: "Demo Bot" }).click();
    await page
      .getByRole("button", { name: "Send to chat", exact: true })
      .click();
    await expect(
      page.getByRole("heading", { name: "Demo Bot" }),
    ).toBeVisible();
    await page.getByRole("button", { name: "Update dummy ticket" }).click();
    await page
      .getByRole("textbox", { name: "Message Demo Bot" })
      .fill("Please investigate");
    await page.getByRole("button", { name: "Send", exact: true }).click();
    await expect(page.getByLabel("Resolved ticket context")).toContainText(
      "Dummy description revision 2",
    );
    await expect(page.getByLabel("Resolved ticket context")).toContainText(
      "Dummy comment revision 2",
    );
    await expect(page.getByLabel("Resolved ticket context")).toContainText(
      "Please investigate",
    );
    if (mergeRequest) {
      await expect(page.getByLabel("Resolved ticket context")).toContainText(
        '"sourceBranch":"feature/context"',
      );
      await expect(page.getByLabel("Resolved ticket context")).toContainText(
        '"targetBranch":"main"',
      );
    }
    expect(errors).toEqual([]);
  });
}

for (const provider of ["jira", "gitlab"]) {
  test(`${provider}: Send to chat defaults to the saved Bot`, async ({ page }) => {
    await page.addInitScript(() => {
      localStorage.setItem("monocode.inboxBots.v1", JSON.stringify({
        jira: "bot-demo",
        gitlab: "bot-demo",
      }));
    });
    await page.goto(
      `/tests/browser/jira-context.html?provider=${provider}&kind=pr`,
    );
    await expect(
      page.getByRole("button", { name: "Choose chat destination" }),
    ).toContainText("Demo Bot");
    await page
      .getByRole("button", { name: "Send to chat", exact: true })
      .click();
    await expect(
      page.getByRole("heading", { name: "Demo Bot" }),
    ).toBeVisible();
    await expect(page.getByLabel("Resolved ticket context")).toContainText(
      "Ticket attached. Add a message",
    );
  });
}

test("opens a project chat with the same ticket card", async ({ page }) => {
  await page.goto("/tests/browser/jira-context.html");
  await page.getByRole("button", { name: "Send to chat", exact: true }).click();
  await expect(
    page.getByRole("heading", { name: "Demo project chat" }),
  ).toBeVisible();
  await expect(
    page.getByRole("button", { name: "Remove Issue DEMO-42" }),
  ).toBeVisible();
});

test("supports keyboard destination selection in a narrow window", async ({ page }, testInfo) => {
  await page.setViewportSize({ width: 560, height: 760 });
  await page.goto("/tests/browser/jira-context.html");
  await page.getByRole("button", { name: "Choose chat destination" }).click();
  const search = page.getByRole("textbox", { name: "Find project or Bot" });
  await expect(search).toBeFocused();
  if (testInfo.project.name === "chromium") await page.screenshot({ path: testInfo.outputPath("ticket-context.png") });
  await search.press("ArrowDown");
  await expect(page.getByRole("option", { name: "Demo project" })).toBeFocused();
  await page.getByRole("option", { name: "Demo project" }).press("ArrowDown");
  await page.getByRole("option", { name: "Demo Bot" }).press("Enter");
  await expect(page.getByRole("button", { name: "Choose chat destination" })).toContainText("Demo Bot");
});
