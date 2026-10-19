import { expect, test, type Page } from "@playwright/test";

test.use({ viewport: { width: 1300, height: 900 } });

async function activateSharedAppearance(page: Page) {
  await page.evaluate(async () => {
    const fixture = window as unknown as {
      translucencyCalls: { command: string; args?: Record<string, unknown> }[];
      __TAURI_INTERNALS__: {
        invoke: (
          command: string,
          args?: Record<string, unknown>,
        ) => Promise<unknown>;
      };
    };
    fixture.translucencyCalls = [];
    const base = fixture.__TAURI_INTERNALS__.invoke;
    fixture.__TAURI_INTERNALS__.invoke = async (command, args) => {
      if (
        command === "set_window_glass_enabled" ||
        command === "set_window_background_blur"
      ) {
        fixture.translucencyCalls.push({ command, args });
        return command === "set_window_glass_enabled" ? true : undefined;
      }
      return base(command, args);
    };
    const path = "/src/features/settings/model/appearance.ts";
    const appearance = await import(path);
    appearance.initAppearance();
    appearance.activateWindowAppearance();
  });
  await page.keyboard.press("Meta+,");
  await page.getByRole("button", { name: "Appearance", exact: true }).click();
}

test("adjusts shared opacity and blur, retaining them through layout, theme, and restart transitions", async ({
  page,
}, testInfo) => {
  test.setTimeout(60_000);
  await page.addInitScript(() => {
    Object.defineProperty(navigator, "platform", {
      value: "MacIntel",
      configurable: true,
    });
    if (!localStorage.getItem("translucency-fixture-initialized")) {
      localStorage.setItem("monocode.insetLayout", "1");
      localStorage.setItem("monocode.colorScheme", "dark");
      localStorage.setItem("monocode.sidebarOpacity", "0.65");
      localStorage.setItem("monocode.sidebarBlur", "38");
      localStorage.setItem("translucency-fixture-initialized", "1");
    }
  });
  await page.goto("/tests/browser/workspace-performance.html");
  await expect(page.locator(".inset-shell")).toBeVisible();
  await activateSharedAppearance(page);
  const opacity = page.getByRole("slider", {
    name: "Window opacity",
    exact: true,
  });
  const blur = page.getByRole("slider", { name: "Blur radius", exact: true });
  await expect(opacity).toHaveValue("65");
  await expect(blur).toHaveValue("38");
  await opacity.press("Home");
  await expect(opacity).toHaveValue("15");
  await opacity.press("End");
  await expect(opacity).toHaveValue("100");
  for (let i = 0; i < 5; i++) await opacity.press("ArrowLeft");
  await blur.press("End");
  for (let i = 0; i < 5; i++) await blur.press("ArrowLeft");
  await expect(opacity).toHaveValue("95");
  await expect(blur).toHaveValue("59");
  const lastNativeValue = (command: string, key: string) =>
    page.evaluate(
      ({ command, key }) => {
        const calls = (
          window as unknown as {
            translucencyCalls: {
              command: string;
              args?: Record<string, unknown>;
            }[];
          }
        ).translucencyCalls;
        return calls.filter((call) => call.command === command).at(-1)?.args?.[
          key
        ];
      },
      { command, key },
    );
  await expect
    .poll(() => lastNativeValue("set_window_glass_enabled", "opacity"))
    .toBe(0.95);
  await expect
    .poll(() => lastNativeValue("set_window_background_blur", "radius"))
    .toBe(59);
  expect(
    await page.evaluate(() =>
      getComputedStyle(document.documentElement)
        .getPropertyValue("--inset-glass-alpha")
        .trim(),
    ),
  ).toBe("0.95");
  await opacity.scrollIntoViewIfNeeded();
  await page.screenshot({ path: testInfo.outputPath("translucency-wide.png") });

  const transparent = page.getByRole("switch", {
    name: "Transparent window",
    exact: true,
  });
  await transparent.click();
  await expect(opacity).toBeDisabled();
  await expect(blur).toBeDisabled();
  await transparent.click();
  await expect(opacity).toBeEnabled();
  const scheme = page.getByRole("radiogroup", { name: "Theme", exact: true });
  await scheme.getByRole("radio", { name: "Light", exact: true }).click();
  await expect(opacity).toBeDisabled();
  await expect(blur).toBeDisabled();
  await scheme.getByRole("radio", { name: "Dark", exact: true }).click();
  await expect(opacity).toBeEnabled();
  await expect(opacity).toHaveValue("95");
  const layout = page.getByRole("switch", { name: "New layout", exact: true });
  await layout.click();
  await expect(
    page.getByRole("slider", { name: "Sidebar opacity", exact: true }),
  ).toHaveValue("95");
  await expect(blur).toHaveValue("59");
  await layout.click();
  await expect(opacity).toHaveValue("95");
  await expect(blur).toHaveValue("59");

  await page.reload();
  await expect(page.locator(".inset-shell")).toBeVisible();
  await activateSharedAppearance(page);
  await expect(opacity).toHaveValue("95");
  await expect(blur).toHaveValue("59");
  await expect
    .poll(() => lastNativeValue("set_window_glass_enabled", "opacity"))
    .toBe(0.95);
  await expect
    .poll(() => lastNativeValue("set_window_background_blur", "radius"))
    .toBe(59);
  await page.setViewportSize({ width: 900, height: 740 });
  await opacity.scrollIntoViewIfNeeded();
  await page.screenshot({
    path: testInfo.outputPath("translucency-narrow.png"),
  });
});

test("keeps the window opaque when its workspace theme or color scheme changes", async ({
  page,
}) => {
  test.setTimeout(60_000);
  await page.addInitScript(() => {
    Object.defineProperty(navigator, "platform", {
      value: "MacIntel",
      configurable: true,
    });
    localStorage.setItem("monocode.insetLayout", "1");
    localStorage.setItem("monocode.colorScheme", "dark");
  });
  await page.goto("/tests/browser/workspace-performance.html");
  await expect(page.locator(".inset-shell")).toBeVisible({ timeout: 15_000 });
  await page.evaluate(async () => {
    const fixture = window as unknown as {
      nativeGlassChanges: boolean[];
      __TAURI_INTERNALS__: {
        invoke: (
          command: string,
          args?: Record<string, unknown>,
        ) => Promise<unknown>;
      };
    };
    fixture.nativeGlassChanges = [];
    const base = fixture.__TAURI_INTERNALS__.invoke;
    fixture.__TAURI_INTERNALS__.invoke = async (command, args) => {
      if (command === "set_window_glass_enabled") {
        fixture.nativeGlassChanges.push(Boolean(args?.enabled));
        return true;
      }
      return base(command, args);
    };
    // The workspace fixture skips the native boot splash; activate its real
    // appearance controller so theme changes exercise the native glass path.
    const path = "/src/features/settings/model/appearance.ts";
    const appearance = await import(path);
    appearance.initAppearance();
    appearance.activateWindowAppearance();
  });
  const root = page.locator("html");
  await expect(root).toHaveClass(/has-native-glass/);
  const modifier = await page.evaluate(() =>
    /Mac|iPhone|iPad/.test(navigator.platform) ? "Meta" : "Control",
  );
  await page.keyboard.press(`${modifier}+,`);
  await page.getByRole("button", { name: "Appearance", exact: true }).click();
  const transparent = page.getByRole("switch", {
    name: "Transparent window",
    exact: true,
  });
  await expect(transparent).toBeChecked();
  await transparent.click();
  await expect(transparent).not.toBeChecked();
  await expect(root).not.toHaveClass(/has-native-glass/);
  await expect
    .poll(() =>
      page.evaluate(() =>
        (
          window as unknown as { nativeGlassChanges: boolean[] }
        ).nativeGlassChanges.at(-1),
      ),
    )
    .toBe(false);

  for (const theme of ["Moss", "Ember"]) {
    await page.getByRole("button", { name: /^Workspace theme:/ }).click();
    await page.getByRole("option", { name: theme, exact: true }).click();
    await expect(transparent).not.toBeChecked();
    await expect(root).not.toHaveClass(/has-native-glass/);
    expect(
      await page.evaluate(() => localStorage.getItem("monocode.insetGlass")),
    ).toBe("0");
    await expect
      .poll(() =>
        page.evaluate(() =>
          (
            window as unknown as { nativeGlassChanges: boolean[] }
          ).nativeGlassChanges.at(-1),
        ),
      )
      .toBe(false);
  }
  const scheme = page.getByRole("radiogroup", { name: "Theme", exact: true });
  await scheme.getByRole("radio", { name: "Light", exact: true }).click();
  await expect(transparent).toBeDisabled();
  await expect(transparent).not.toBeChecked();
  await scheme.getByRole("radio", { name: "Dark", exact: true }).click();
  await expect(transparent).toBeEnabled();
  await expect(transparent).not.toBeChecked();
  await expect(root).not.toHaveClass(/has-native-glass/);

  await transparent.click();
  await expect(transparent).toBeChecked();
  await expect(root).toHaveClass(/has-native-glass/);
  await expect
    .poll(() =>
      page.evaluate(() =>
        (
          window as unknown as { nativeGlassChanges: boolean[] }
        ).nativeGlassChanges.at(-1),
      ),
    )
    .toBe(true);
});
