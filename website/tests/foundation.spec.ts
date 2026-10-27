import { test, expect } from "@playwright/test";
import AxeBuilder from "@axe-core/playwright";

const home = process.env.SITE_BASE || "/Radivine/";
test("theme preference, OS changes, persistence and cross-tab storage", async ({
  page,
  context,
}) => {
  await page.emulateMedia({ colorScheme: "dark" });
  await page.goto(home);
  const root = page.locator("html");
  await expect(root).toHaveAttribute("data-theme", "dark");
  await expect(page.getByLabel("Website theme")).toHaveValue("system");
  await page.getByLabel("Website theme").selectOption("light");
  await page.reload();
  await expect(root).toHaveAttribute("data-theme", "light");
  await page.emulateMedia({ colorScheme: "light" });
  await page.getByLabel("Website theme").selectOption("dark");
  await expect(root).toHaveAttribute("data-theme", "dark");
  const other = await context.newPage();
  await other.goto(home);
  await other.getByLabel("Website theme").selectOption("system");
  await expect(root).toHaveAttribute("data-preference", "system");
  await expect(root).toHaveAttribute("data-theme", "light");
  await page.emulateMedia({ colorScheme: "dark" });
  await expect(root).toHaveAttribute("data-theme", "dark");
  await page.goto(`${home}404.html`);
  await expect(root).toHaveAttribute("data-preference", "system");
});

test("early theme sets attributes before stylesheet, invalid and denied storage are tolerated", async ({
  browser,
}) => {
  for (const mode of ["dark", "invalid", "blocked"]) {
    const context = await browser.newContext({ colorScheme: "light" });
    const page = await context.newPage();
    await page.addInitScript((mode) => {
      if (mode === "blocked")
        Object.defineProperty(window, "localStorage", {
          get() {
            throw new Error("Denied");
          },
        });
      else localStorage.setItem("radivine:theme", mode);
    }, mode);
    const errors: string[] = [];
    page.on("pageerror", (error) => errors.push(error.message));
    await page.route("**/*.css", (route) => route.abort());
    await page.goto(home);
    await expect(page.locator("html")).toHaveAttribute(
      "data-theme",
      mode === "dark" ? "dark" : "light",
    );
    if (mode === "blocked") {
      await page.getByLabel("Website theme").selectOption("dark");
      await expect(page.locator("html")).toHaveAttribute("data-theme", "dark");
    }
    expect(errors).toEqual([]);
    await context.close();
  }
});

for (const theme of ["light", "dark"] as const) {
  for (const width of [320, 390, 768, 1440]) {
    test(`${theme} at ${width}px: responsive, assets, accessibility`, async ({
      page,
    }) => {
      await page.setViewportSize({ width, height: 960 });
      await page.emulateMedia({ colorScheme: theme, reducedMotion: "reduce" });
      const errors: string[] = [];
      page.on("pageerror", (error) => errors.push(error.message));
      page.on("response", (response) => {
        if (response.status() >= 400) errors.push(response.url());
      });
      await page.goto(home);
      await page.evaluate(() => document.fonts.ready);
      expect(
        await page.evaluate(
          () => document.documentElement.scrollWidth <= window.innerWidth,
        ),
      ).toBe(true);
      await expect(page.getByRole("heading", { level: 1 })).toHaveText(
        "Branch intoaction.",
      );
      await page.locator("img").evaluateAll((images) =>
        images.forEach((image) => {
          if (image instanceof HTMLImageElement) image.loading = "eager";
        }),
      );
      await expect
        .poll(() =>
          page
            .locator("img")
            .evaluateAll((images) =>
              images.every(
                (image) =>
                  image instanceof HTMLImageElement &&
                  image.complete &&
                  image.naturalWidth > 0,
              ),
            ),
        )
        .toBe(true);
      const results = await new AxeBuilder({ page }).analyze();
      expect(results.violations).toEqual([]);
      expect(errors).toEqual([]);
      expect(
        await page.evaluate(
          () => getComputedStyle(document.documentElement).scrollBehavior,
        ),
      ).toBe("auto");
      await page.screenshot({
        path: `test-results/home-${theme}-${width}.png`,
        fullPage: true,
      });
    });
  }
}

test("no-JavaScript mobile disclosure and system theme remain usable", async ({
  browser,
}) => {
  const context = await browser.newContext({
    javaScriptEnabled: false,
    colorScheme: "dark",
    viewport: { width: 320, height: 800 },
  });
  const page = await context.newPage();
  await page.goto(home);
  await page.locator("summary").click();
  await expect(
    page
      .getByRole("navigation", { name: "Mobile navigation" })
      .getByText("Compatibility"),
  ).toBeVisible();
  await expect(page.getByRole("link", { name: "Get Radivine" })).toBeVisible();
  expect(
    await page
      .locator("body")
      .evaluate((element) => getComputedStyle(element).backgroundColor),
  ).toBe("rgb(10, 26, 22)");
  await page
    .getByRole("navigation", { name: "Mobile navigation" })
    .getByText("Compatibility")
    .click();
  await expect(page).toHaveURL(new RegExp(`${home}compatibility/$`));
  await context.close();
});

test("keyboard skip, focus visibility, mobile Escape and 200% equivalent layout", async ({
  page,
}) => {
  await page.setViewportSize({ width: 640, height: 480 });
  await page.goto(home);
  await page.keyboard.press("Tab");
  await expect(
    page.getByRole("link", { name: "Skip to content" }),
  ).toBeFocused();
  await page.keyboard.press("Enter");
  await expect(page.locator("#main")).toBeFocused();
  await page.locator("summary").click();
  const navigation = page.getByRole("navigation", {
    name: "Mobile navigation",
  });
  await page.keyboard.press("Tab");
  await expect(navigation.getByText("Product")).toBeFocused();
  expect(
    await navigation
      .getByText("Product")
      .evaluate((element) => getComputedStyle(element).outlineStyle),
  ).toBe("solid");
  await page.keyboard.press("Escape");
  await expect(page.locator("summary")).toBeFocused();
  await expect(navigation).toBeHidden();
  expect(
    await page.evaluate(
      () => document.documentElement.scrollWidth <= innerWidth,
    ),
  ).toBe(true);
});

test("404 keeps base-aware home and metadata", async ({ page }) => {
  await page.goto(`${home}404.html`);
  await expect(
    page.getByRole("link", { name: "Back to Radivine" }),
  ).toHaveAttribute("href", home);
  await expect(page.locator('meta[name="robots"]')).toHaveAttribute(
    "content",
    "noindex",
  );
  expect((await new AxeBuilder({ page }).analyze()).violations).toEqual([]);
});
