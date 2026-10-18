import { expect, test, type Page } from "@playwright/test";

test("vox settings persists the microphone choice for the next call", async ({
  page,
}, testInfo) => {
  const errors: string[] = [];
  page.on("pageerror", (error) => errors.push(error.message));
  await page.goto("/tests/browser/cli-providers.html?section=vox");
  await expect(
    page.getByRole("heading", { name: "Vox", exact: true }),
  ).toBeVisible();
  const selector = page.getByRole("button", {
    name: "Default microphone: MacBook Pro Microphone",
    exact: true,
  });
  await expect(selector).toBeVisible();
  await selector.click();
  await expect(
    page.getByRole("option", { name: "MacBook Pro Microphone", exact: true }),
  ).toBeVisible();
  await expect(page.getByRole("option")).toHaveText([
    "MacBook Pro Microphone",
    "AirPods Pro",
  ]);
  await page.screenshot({
    path: testInfo.outputPath("vox-settings.png"),
    animations: "disabled",
  });
  await page.getByRole("option", { name: "AirPods Pro", exact: true }).click();
  await page.reload();
  await expect(
    page.getByRole("button", {
      name: "Default microphone: AirPods Pro",
      exact: true,
    }),
  ).toBeVisible();
  await page.goto("/tests/browser/vox.html");
  await page.getByRole("button", { name: "vox", exact: true }).click();
  await expect
    .poll(() => page.evaluate(() => (window as any).voxMetrics.captures))
    .toBe(1);
  expect(
    await page.evaluate(() => (window as any).voxMetrics.captureInputs[0]),
  ).toHaveProperty("deviceId", { exact: "airpods" });
  await endVox(page);
  expect(errors).toEqual([]);
});

async function endVox(page: Page) {
  if (
    !(await page.getByRole("dialog", { name: "vox", exact: true }).isVisible())
  )
    await page.getByRole("button", { name: "vox", exact: true }).click();
  await page.getByRole("button", { name: "end", exact: true }).click();
}

test("voice starts and stops without touching the draft or sending audio over IPC", async ({
  page,
}, testInfo) => {
  await page.goto("/tests/browser/vox.html");
  const input = page.locator("textarea");
  await input.fill("Keep this unfinished message");
  await expect(
    page.getByRole("button", { name: "vox", exact: true }),
  ).toBeVisible();
  expect(await page.evaluate(() => (window as any).voxMetrics.captures)).toBe(
    0,
  );
  await page.getByRole("button", { name: "vox", exact: true }).click();
  await expect(
    page.getByRole("status").filter({ hasText: "connecting…" }),
  ).toBeVisible();
  await expect
    .poll(() =>
      page.evaluate(() =>
        (window as any).voxMetrics.writes.includes("thread/realtime/start"),
      ),
    )
    .toBe(true);
  await page.screenshot({ path: testInfo.outputPath("connecting.png") });
  await page.evaluate(() => (window as any).connectVox());
  await expect(
    page.getByRole("status").filter({ hasText: "listening" }),
  ).toBeVisible();
  await page.screenshot({ path: testInfo.outputPath("live.png") });
  await endVox(page);
  await expect(
    page.getByRole("button", { name: "vox", exact: true }),
  ).toBeVisible();
  await expect(input).toHaveValue("Keep this unfinished message");
  const metrics = await page.evaluate(() => (window as any).voxMetrics);
  expect(metrics).toMatchObject({
    captures: 1,
    stopped: 1,
    closed: 1,
    spawned: 1,
    killed: 1,
  });
  expect(metrics.writes).toEqual([
    "initialize",
    "initialized",
    "thread/start",
    "thread/realtime/start",
  ]);
});

test("a service failure releases the call and shows an actionable error in either theme", async ({
  page,
}, testInfo) => {
  for (const theme of ["dark", "light"]) {
    await page.goto(`/tests/browser/vox.html?theme=${theme}`);
    await page.getByRole("button", { name: "vox", exact: true }).click();
    await expect
      .poll(() => page.evaluate(() => (window as any).voxMetrics.spawned))
      .toBe(1);
    await expect
      .poll(() =>
        page.evaluate(() =>
          (window as any).voxMetrics.writes.includes("thread/realtime/start"),
        ),
      )
      .toBe(true);
    await page.evaluate(() => (window as any).failVox());
    await expect(page.getByRole("alert")).toContainText(
      "chatgpt voice is unavailable",
    );
    await page.screenshot({
      path: testInfo.outputPath(`error-${theme}.png`),
      animations: "disabled",
    });
    await expect
      .poll(() => page.evaluate(() => (window as any).voxMetrics.killed))
      .toBe(1);
    await page
      .getByRole("button", { name: "close vox panel", exact: true })
      .click();
    await expect(page.getByRole("alert")).toHaveCount(0);
  }
});

test("Vox survives chat navigation and titlebar replacement", async ({
  page,
}) => {
  await page.goto("/tests/browser/vox.html");
  await page.getByRole("button", { name: "vox", exact: true }).click();
  await expect
    .poll(() =>
      page.evaluate(() =>
        (window as any).voxMetrics.writes.includes("thread/realtime/start"),
      ),
    )
    .toBe(true);
  await page.evaluate(() => (window as any).connectVox());
  for (const action of [
    "Switch chat",
    "Hide chat",
    "Remote chat",
    "Move controls",
  ]) {
    await page.getByRole("button", { name: action, exact: true }).click();
    await expect(
      page.getByRole("button", { name: "vox", exact: true }),
    ).toHaveAttribute("aria-pressed", "true");
  }
  expect(await page.evaluate(() => (window as any).voxMetrics)).toMatchObject({
    captures: 1,
    stopped: 0,
    spawned: 1,
    killed: 0,
  });
  await endVox(page);
  await expect
    .poll(() => page.evaluate(() => (window as any).voxMetrics.killed))
    .toBe(1);
  expect(await page.evaluate(() => (window as any).voxMetrics.stopped)).toBe(1);
});

test("voice passes messages to explicit session and Mono targets while keeping the draft", async ({
  page,
}) => {
  await page.goto("/tests/browser/vox.html");
  await page.locator("textarea").fill("Unfinished typed draft");
  await page.getByRole("button", { name: "vox", exact: true }).click();
  await expect
    .poll(() =>
      page.evaluate(() =>
        (window as any).voxMetrics.writes.includes("thread/realtime/start"),
      ),
    )
    .toBe(true);
  await page.evaluate(() =>
    (window as any).voxAction("session-call", "sessions.send", {
      project: "/other",
      sessionId: "other-session",
      prompt: "Fix its tests",
    }),
  );
  await page.getByRole("button", { name: "Switch chat", exact: true }).click();
  await page.evaluate(() =>
    (window as any).voxAction("mono-call", "monos.send", {
      to: "Helper",
      message: "Review the change",
      expectReply: false,
    }),
  );
  await expect
    .poll(() =>
      page.evaluate(() => (window as any).voxMetrics.responses.length),
    )
    .toBe(2);
  expect(await page.evaluate(() => (window as any).voxMetrics.actions)).toEqual(
    [
      {
        action: "sessions.send",
        input: {
          project: "/other",
          sessionId: "other-session",
          prompt: "Fix its tests",
        },
      },
      {
        action: "monos.send",
        input: {
          to: "Helper",
          message: "Review the change",
          expectReply: false,
        },
      },
    ],
  );
  await expect(page.locator("textarea")).toHaveValue("Unfinished typed draft");
  await endVox(page);
});

test("voice fills session and controlled Mono composers without sending or erasing typing", async ({
  page,
}) => {
  await page.goto("/tests/browser/vox.html");
  await page.locator("textarea").fill("Existing draft");
  await page.getByRole("button", { name: "vox", exact: true }).click();
  await expect
    .poll(() =>
      page.evaluate(() =>
        (window as any).voxMetrics.writes.includes("thread/realtime/start"),
      ),
    )
    .toBe(true);
  await page.evaluate(() =>
    (window as any).voxAction("fill-session", "composer.write", {
      text: "Review the latest change",
    }),
  );
  await expect(page.locator("textarea")).toHaveValue(
    "Existing draft\n\nReview the latest change",
  );
  await expect
    .poll(() =>
      page.evaluate(() => (window as any).voxMetrics.responses.length),
    )
    .toBe(1);
  expect(await page.evaluate(() => (window as any).voxMetrics.submitted)).toBe(
    0,
  );
  await page.keyboard.press("Escape");
  await page
    .getByRole("button", { name: "Mono composer", exact: true })
    .click();
  const mono = page.getByRole("textbox", { name: "Message Helper" });
  await mono.fill("Keep my Mono draft");
  await page.evaluate(() =>
    (window as any).voxAction("fill-mono", "composer.write", {
      text: "Please investigate",
    }),
  );
  await expect(mono).toHaveValue("Keep my Mono draft\n\nPlease investigate");
  await page
    .getByRole("button", { name: "Move controls", exact: true })
    .click();
  await expect(mono).toHaveValue("Keep my Mono draft\n\nPlease investigate");
  await mono.fill("Manual edit wins");
  await page.evaluate(() =>
    (window as any).voxAction("stale-replace", "composer.write", {
      text: "Do not overwrite",
      mode: "replace",
      expectedText: "Keep my Mono draft",
    }),
  );
  await expect
    .poll(() =>
      page.evaluate(() => (window as any).voxMetrics.responses.length),
    )
    .toBe(3);
  await expect(mono).toHaveValue("Manual edit wins");
  expect(await page.evaluate(() => (window as any).voxMetrics.submitted)).toBe(
    0,
  );
  await endVox(page);
});

test("native final replies, silent replies, and mic control share the existing connection", async ({
  page,
}, testInfo) => {
  await page.goto("/tests/browser/vox.html");
  await page.getByRole("button", { name: "vox", exact: true }).click();
  const panel = page.getByRole("dialog", { name: "vox", exact: true });
  await expect
    .poll(() =>
      page.evaluate(() =>
        (window as any).voxMetrics.writes.includes("thread/realtime/start"),
      ),
    )
    .toBe(true);
  await page.evaluate(() => (window as any).connectVox());
  await page.evaluate(() => {
    const notify = (window as any).voxNotify;

    notify("thread/realtime/transcript/delta", {
      role: "user",
      delta: "Please review OriginalCase.ts",
    });
    notify("thread/realtime/transcript/done", {
      role: "user",
      text: "Please review FinalCase.ts",
    });
    notify("turn/started", {});
  });
  await expect(
    panel.getByText("Please review FinalCase.ts", { exact: true }),
  ).toHaveCount(0);
  await expect(panel.locator("[data-vox-face]")).toHaveAttribute(
    "data-vox-face",
    "thinking",
  );
  await page
    .getByRole("button", { name: "silent replies", exact: true })
    .click();
  expect(await page.evaluate(() => (window as any).voxNativeState())).toEqual({
    muted: true,
    micEnabled: true,
  });
  await page.getByRole("button", { name: "mic on", exact: true }).click();
  expect(await page.evaluate(() => (window as any).voxNativeState())).toEqual({
    muted: true,
    micEnabled: false,
  });
  await expect(panel.locator("[data-vox-face]")).toHaveAttribute(
    "data-vox-face",
    "paused",
  );
  await page.getByRole("button", { name: "mic off", exact: true }).click();
  await page.evaluate(() => {
    const notify = (window as any).voxNotify;
    notify("turn/completed", {});

    notify("thread/realtime/transcript/delta", {
      role: "assistant",
      delta: "working on it",
    });
    notify("thread/realtime/transcript/done", {
      role: "assistant",
      text: "message accepted by helper",
    });
  });
  await expect(
    panel.getByText("message accepted by helper", { exact: true }),
  ).toBeVisible();
  const box = await panel.boundingBox();
  expect(Math.abs(box!.width - box!.height)).toBeLessThan(3);
  await page.screenshot({
    path: testInfo.outputPath("panel-native-captions.png"),
    animations: "disabled",
  });
  await panel.press("Escape");
  await expect(panel).toHaveCount(0);
  await page.evaluate(() => {
    const notify = (window as any).voxNotify;

    notify("thread/realtime/transcript/done", {
      role: "assistant",
      text: "captions retained while hidden",
    });
  });
  await page.getByRole("button", { name: "vox", exact: true }).click();
  await expect(
    panel.getByText("captions retained while hidden", { exact: true }),
  ).toBeVisible();
  expect(await page.evaluate(() => (window as any).voxMetrics)).toMatchObject({
    captures: 1,
    spawned: 1,
    stopped: 0,
  });
  expect(await page.evaluate(() => (window as any).voxNativeState())).toEqual({
    muted: true,
    micEnabled: true,
  });
  await endVox(page);
});
