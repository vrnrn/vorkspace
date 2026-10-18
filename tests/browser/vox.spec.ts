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
  const button = page.getByRole("button", { name: "vox", exact: true });
  if ((await button.getAttribute("aria-pressed")) === "true")
    await button.click();
  await expect(button).toHaveAttribute("aria-pressed", "false");
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
    "thread/realtime/stop",
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
    await page.locator("textarea").click();
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
  const control = page.getByRole("button", { name: "vox", exact: true });
  for (const action of [
    "Switch chat",
    "Hide chat",
    "Remote chat",
    "Move controls",
  ]) {
    await page.getByRole("button", { name: action, exact: true }).click();
    await expect(control).toHaveAttribute("aria-pressed", "true");
    await expect(control.locator("[data-vox-motion]")).toHaveAttribute(
      "data-vox-motion",
      "idle",
    );
  }
  await page
    .getByRole("button", { name: "Toggle controls", exact: true })
    .click();
  await expect(control).toHaveCount(0);
  expect(
    await page.evaluate(
      () =>
        document
          .getAnimations()
          .filter((a) => (a as CSSAnimation).animationName?.startsWith("vox-"))
          .length,
    ),
  ).toBe(0);
  await page
    .getByRole("button", { name: "Toggle controls", exact: true })
    .click();
  await expect(control).toHaveAttribute("aria-pressed", "true");
  await page.setViewportSize({ width: 800, height: 640 });
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

test("silent replies live in Settings and use a bubble below the animated titlebar face on the same connection", async ({
  page,
}, testInfo) => {
  await page.goto("/tests/browser/vox.html");
  const control = page.getByRole("button", { name: "vox", exact: true });
  await expect(control.locator("[data-vox-motion]")).toHaveText("vox");
  expect(
    await page.evaluate(
      () =>
        document
          .getAnimations()
          .filter((a) => (a as CSSAnimation).animationName?.startsWith("vox-"))
          .length,
    ),
  ).toBe(0);
  await control.click();
  await expect
    .poll(() =>
      page.evaluate(() =>
        (window as any).voxMetrics.writes.includes("thread/realtime/start"),
      ),
    )
    .toBe(true);
  await page.evaluate(() => (window as any).connectVox());
  const face = control.locator("[data-vox-motion]");
  await expect(face).toHaveAttribute("data-vox-motion", "idle");
  await expect(
    page.getByRole("dialog", { name: "vox", exact: true }),
  ).toHaveCount(0);
  await expect(
    page.getByRole("button", {
      name: /^(mic on|mic off|silent replies|end)$/,
      exact: true,
    }),
  ).toHaveCount(0);
  await page.evaluate(() => {
    (window as any).voxNotify("thread/realtime/transcript/done", {
      role: "user",
      text: "private input",
    });
    (window as any).voxNotify("turn/started", {});
  });
  await expect(face).toHaveAttribute("data-vox-motion", "thinking");
  await page.getByRole("button", { name: "Vox settings", exact: true }).click();
  const silent = page.getByRole("switch", {
    name: "Silent replies",
    exact: true,
  });
  await silent.click();
  expect(await page.evaluate(() => (window as any).voxNativeState())).toEqual({
    muted: true,
    micEnabled: true,
  });
  await page.screenshot({
    path: testInfo.outputPath("vox-titlebar-settings.png"),
    animations: "disabled",
  });
  await page.keyboard.press("Escape");
  await page.evaluate(() => {
    (window as any).voxRealtime({
      type: "output_transcript.added",
      start_ms: 1000,
    });
    (window as any).voxNotify("turn/completed", {});
    (window as any).voxNotify("thread/realtime/transcript/done", {
      role: "assistant",
      text: "draft ready in helper’s composer. not sent.",
    });
  });
  await expect(face).toHaveAttribute("data-vox-motion", "talking");
  const bubble = page.getByRole("status", { name: "vox reply", exact: true });
  await expect(bubble).toHaveText(
    "draft ready in helper’s composer. not sent.",
  );
  const buttonBox = (await control.boundingBox())!;
  const bubbleBox = (await bubble.boundingBox())!;
  expect(bubbleBox.y).toBeGreaterThanOrEqual(buttonBox.y + buttonBox.height);
  expect(bubbleBox.height).toBeLessThan(90);
  await expect(page.getByText("private input", { exact: true })).toHaveCount(0);
  await page.screenshot({
    path: testInfo.outputPath("vox-titlebar-silent.png"),
    animations: "disabled",
  });
  await page.evaluate(() =>
    (window as any).voxNotify("thread/realtime/transcript/done", {
      role: "assistant",
      text:
        Array.from(
          { length: 30 },
          (_, i) => `reply line ${i + 1}: the draft is ready for review.`,
        ).join("\n") + "\nthis is the final sentence.",
    }),
  );
  await expect(bubble).toContainText("this is the final sentence.");
  expect((await bubble.boundingBox())!.height).toBeLessThanOrEqual(180);
  expect(await bubble.evaluate((el) => el.scrollHeight > el.clientHeight)).toBe(
    true,
  );
  await bubble.hover();
  await page.mouse.wheel(0, 3000);
  await expect
    .poll(() =>
      bubble.evaluate(
        (el) =>
          el.scrollTop > 0 &&
          el.scrollTop + el.clientHeight >= el.scrollHeight - 2,
      ),
    )
    .toBe(true);
  await page.screenshot({
    path: testInfo.outputPath("vox-titlebar-long.png"),
    animations: "disabled",
  });
  await page.locator("textarea").click();
  await expect(bubble).toHaveCount(0);
  await page.evaluate(() =>
    (window as any).voxNotify("thread/realtime/transcript/done", {
      role: "assistant",
      text: "only the latest reply stays",
    }),
  );
  await expect(bubble).toHaveText("only the latest reply stays");
  await page.keyboard.press("Escape");
  await expect(bubble).toHaveCount(0);
  await page.getByRole("button", { name: "Vox settings", exact: true }).click();
  await silent.click();
  expect(await page.evaluate(() => (window as any).voxNativeState())).toEqual({
    muted: false,
    micEnabled: true,
  });
  await page.keyboard.press("Escape");
  await expect(bubble).toHaveCount(0);
  expect(await page.evaluate(() => (window as any).voxMetrics)).toMatchObject({
    captures: 1,
    spawned: 1,
    stopped: 0,
  });
  await endVox(page);
  await expect(face).toHaveAttribute("data-vox-motion", "off");
  expect(
    await page.evaluate(
      () =>
        document
          .getAnimations()
          .filter((a) => (a as CSSAnimation).animationName?.startsWith("vox-"))
          .length,
    ),
  ).toBe(0);
  await page.screenshot({
    path: testInfo.outputPath("vox-titlebar-off.png"),
    animations: "disabled",
  });
});

test("the titlebar face cycles glyphs only during a call and respects reduced motion", async ({
  page,
}, testInfo) => {
  await page.goto("/tests/browser/vox.html");
  const control = page.getByRole("button", { name: "vox", exact: true });
  await control.click();
  await expect
    .poll(() =>
      page.evaluate(() =>
        (window as any).voxMetrics.writes.includes("thread/realtime/start"),
      ),
    )
    .toBe(true);
  await page.evaluate(() => (window as any).connectVox());
  const face = control.locator("[data-vox-motion]");
  await expect(face).toHaveAttribute("data-vox-motion", "idle");
  const samples = async (times: number[]) =>
    page.evaluate((times) => {
      const field = document.querySelector("[data-vox-motion]")!;
      const animations = field.getAnimations({ subtree: true });
      return times.map((time) => {
        animations.forEach((a) => {
          a.pause();
          a.currentTime = time;
        });
        return Array.from(field.children)
          .filter((el) => Number(getComputedStyle(el).opacity) > 0.5)
          .map((el) => el.textContent);
      });
    }, times);
  const idle = await samples([0, 6300, 6720]);
  expect(idle.every((frame) => frame.length === 1)).toBe(true);
  expect(new Set(idle.flat()).size).toBe(3);
  await page.evaluate(() =>
    (window as any).voxRealtime({
      type: "output_transcript.added",
      start_ms: 1000,
    }),
  );
  await expect(face).toHaveAttribute("data-vox-motion", "talking");
  const talking = await samples([0, 250, 450, 650]);
  expect(talking.every((frame) => frame.length === 1)).toBe(true);
  expect(new Set(talking.flat()).size).toBeGreaterThanOrEqual(3);
  await page.screenshot({
    path: testInfo.outputPath("vox-titlebar-talking.png"),
    animations: "disabled",
  });
  await page.emulateMedia({ reducedMotion: "reduce" });
  expect(
    await page.evaluate(
      () =>
        document
          .getAnimations()
          .filter((a) => (a as CSSAnimation).animationName?.startsWith("vox-"))
          .length,
    ),
  ).toBe(0);
  await expect(face).toHaveAttribute("data-vox-motion", "talking");
  await endVox(page);
  await expect(face).toHaveText("vox");
  await page.emulateMedia({ reducedMotion: "no-preference" });
  expect(
    await page.evaluate(
      () =>
        document
          .getAnimations()
          .filter((a) => (a as CSSAnimation).animationName?.startsWith("vox-"))
          .length,
    ),
  ).toBe(0);
});

test("Vox retains its conversation across reloads and manages its own memories in settings", async ({
  page,
}, testInfo) => {
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
  await page.evaluate(() =>
    (window as any).voxAction("remember", "memory.add", {
      fact: "Prefers brief spoken replies",
    }),
  );
  await expect
    .poll(() =>
      page.evaluate(() => (window as any).voxMetrics.responses.length),
    )
    .toBe(1);
  await endVox(page);
  await page.reload();
  await page.getByRole("button", { name: "vox", exact: true }).click();
  await expect
    .poll(() =>
      page.evaluate(() =>
        (window as any).voxMetrics.writes.includes("thread/realtime/start"),
      ),
    )
    .toBe(true);
  expect(
    await page.evaluate(() => (window as any).voxMetrics.writes),
  ).toContain("thread/resume");
  expect(
    await page.evaluate(() => (window as any).voxMetrics.writes),
  ).not.toContain("thread/start");
  await page.evaluate(() => (window as any).connectVox());
  await page.getByRole("button", { name: "Vox settings", exact: true }).click();
  await expect(
    page.getByRole("heading", { name: "Vox", exact: true }),
  ).toBeVisible();
  await expect(
    page.getByText("Prefers brief spoken replies", { exact: true }),
  ).toBeVisible();
  await page.getByRole("button", { name: "Add memory", exact: true }).click();
  await page
    .getByRole("textbox", { name: "New memory", exact: true })
    .fill("Uses pnpm for personal projects");
  await page
    .getByRole("textbox", { name: "New memory", exact: true })
    .press("Enter");
  await page
    .getByRole("textbox", { name: "New memory", exact: true })
    .press("Escape");
  await expect(
    page.getByText("Uses pnpm for personal projects", { exact: true }),
  ).toBeVisible();
  await page
    .getByRole("button", { name: "Reset conversation", exact: true })
    .click();
  await expect(
    page.getByRole("dialog", { name: "vox", exact: true }),
  ).toHaveCount(0);
  await expect
    .poll(() =>
      page.evaluate(
        () =>
          JSON.parse(localStorage.getItem("vorkspace.vox.conversation.v1")!)
            .threadId ?? null,
      ),
    )
    .toBe(null);
  await expect(
    page.getByText("Prefers brief spoken replies", { exact: true }),
  ).toBeVisible();
  await page.screenshot({
    path: testInfo.outputPath("vox-memory-settings.png"),
    animations: "disabled",
  });
  await page
    .getByRole("button", {
      name: "Forget: Prefers brief spoken replies",
      exact: true,
    })
    .click();
  await expect(
    page.getByText("Prefers brief spoken replies", { exact: true }),
  ).toHaveCount(0);
  await expect(
    page.getByText("Uses pnpm for personal projects", { exact: true }),
  ).toBeVisible();
  await page.keyboard.press("Escape");
  await page.getByRole("button", { name: "vox", exact: true }).click();
  await expect
    .poll(() =>
      page.evaluate(
        () =>
          (window as any).voxMetrics.writes.filter(
            (m: string) => m === "thread/start",
          ).length,
      ),
    )
    .toBe(1);
  await endVox(page);
  await page.reload();
  await page.getByRole("button", { name: "Vox settings", exact: true }).click();
  await expect(
    page.getByText("Prefers brief spoken replies", { exact: true }),
  ).toHaveCount(0);
  await expect(
    page.getByText("Uses pnpm for personal projects", { exact: true }),
  ).toBeVisible();
});

test("a failed Vox reset preserves the conversation and shows recovery beside Conversation", async ({
  page,
}, testInfo) => {
  await page.goto("/tests/browser/cli-providers.html?section=vox");
  await page.evaluate(() => {
    localStorage.setItem(
      "vorkspace.vox.conversation.v1",
      JSON.stringify({ revision: "test", threadId: "retained-thread" }),
    );
    const original = Storage.prototype.setItem;
    Storage.prototype.setItem = function (key, value) {
      if (key === "vorkspace.vox.conversation.v1")
        throw new DOMException("storage full", "QuotaExceededError");
      return original.call(this, key, value);
    };
  });
  await page
    .getByRole("button", { name: "Reset conversation", exact: true })
    .click();
  const conversation = page.locator("section").filter({
    has: page.getByRole("heading", { name: "Conversation", exact: true }),
  });
  await expect(conversation.getByRole("alert")).toHaveText(
    "Could not reset Vox's conversation. Try again.",
  );
  expect(
    await page.evaluate(
      () =>
        JSON.parse(localStorage.getItem("vorkspace.vox.conversation.v1")!)
          .threadId,
    ),
  ).toBe("retained-thread");
  await expect(
    page.getByRole("button", { name: "Reset conversation", exact: true }),
  ).toBeEnabled();
  await page.screenshot({
    path: testInfo.outputPath("vox-reset-error.png"),
    animations: "disabled",
  });
});

test("Vox opt-in is the first setting, defaults off and explains the live voice requirement", async ({
  page,
}, testInfo) => {
  await page.goto("/tests/browser/cli-providers.html?section=vox");
  const toggle = page.getByRole("switch", { name: "Enable Vox", exact: true });
  await expect(toggle).not.toBeChecked();
  await expect(page.getByRole("switch").first()).toHaveAccessibleName(
    "Enable Vox",
  );
  await expect(
    page.getByText("Requires Codex with ChatGPT live voice access.", {
      exact: true,
    }),
  ).toBeVisible();
  await page.screenshot({
    path: testInfo.outputPath("vox-opt-in-settings.png"),
    animations: "disabled",
  });
  const silent = page.getByRole("switch", {
    name: "Silent replies",
    exact: true,
  });
  await expect(silent).not.toBeChecked();
  await silent.click();
  await page.reload();
  await expect(silent).toBeChecked();
  await expect(toggle).not.toBeChecked();
  await toggle.click();
  await expect(toggle).toBeChecked();
  await page.reload();
  await expect(toggle).toBeChecked();
  await toggle.click();
  await page.reload();
  await expect(toggle).not.toBeChecked();
});

test("disabling Vox ends the in-app call without remounting settings or resetting its conversation", async ({
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
  const saved = await page.evaluate(() =>
    localStorage.getItem("vorkspace.vox.conversation.v1"),
  );
  await page.getByRole("button", { name: "Vox settings", exact: true }).click();
  const toggle = page.getByRole("switch", { name: "Enable Vox", exact: true });
  await toggle.click();
  await expect(toggle).not.toBeChecked();
  await expect(toggle).toBeEnabled();
  await expect(
    page.getByRole("dialog", { name: "vox", exact: true }),
  ).toHaveCount(0);
  await expect
    .poll(() => page.evaluate(() => (window as any).voxMetrics.killed))
    .toBe(1);
  expect(await page.evaluate(() => (window as any).voxMetrics)).toMatchObject({
    captures: 1,
    stopped: 1,
    closed: 1,
  });
  expect(
    await page.evaluate(() =>
      localStorage.getItem("vorkspace.vox.conversation.v1"),
    ),
  ).toBe(saved);
  await toggle.click();
  await expect(toggle).toBeChecked();
  await expect(
    page.getByRole("heading", { name: "Vox", exact: true }),
  ).toBeVisible();
  expect(await page.evaluate(() => (window as any).voxMetrics.captures)).toBe(
    1,
  );
});
