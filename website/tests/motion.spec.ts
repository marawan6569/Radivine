import { test, expect } from "@playwright/test";

const home = process.env.SITE_BASE || "/Radivine/";
test.use({
  viewport: { width: 1440, height: 960 },
  reducedMotion: "no-preference",
});

async function settle(page: import("@playwright/test").Page) {
  await expect
    .poll(() =>
      page.evaluate(
        () =>
          document
            .getAnimations()
            .filter((animation) => animation.playState === "running").length,
      ),
    )
    .toBe(0);
}

test("scroll story follows all four stages in both directions without hiding copy", async ({
  page,
}) => {
  await page.goto(home);
  const story = page.locator("[data-story]");
  const steps = page.locator("[data-story-step]");
  await expect(steps).toHaveCount(4);
  for (const index of [0, 1, 2, 3, 2, 1, 0]) {
    await steps
      .nth(index)
      .evaluate((element) =>
        element.scrollIntoView({ block: "center", behavior: "instant" }),
      );
    await expect(story).toHaveAttribute("data-step", String(index));
    await expect(steps.nth(index)).toHaveClass(/is-current/);
    await expect(steps.nth(index).locator("h3")).toBeVisible();
    await settle(page);
    expect(
      await story
        .locator(".root-wheel")
        .evaluate((element) => getComputedStyle(element).opacity),
    ).toBe(index === 0 ? "0" : "1");
    expect(
      await story
        .locator(".child-wheel")
        .evaluate((element) => getComputedStyle(element).opacity),
    ).toBe(index < 2 ? "0" : "1");
    expect(
      await story
        .locator(".story-done")
        .evaluate((element) => getComputedStyle(element).opacity),
    ).toBe(index === 3 ? "1" : "0");
  }
});

test("desktop pointer is bounded, resets, and live reduced motion removes enhancements", async ({
  page,
}) => {
  await page.goto(home);
  const art = page.locator("[data-pointer-art]");
  await page.mouse.move(1100, 400);
  await expect
    .poll(() =>
      art.evaluate((element) => element.style.getPropertyValue("--pointer-x")),
    )
    .not.toBe("");
  const x = await art.evaluate((element) =>
    parseFloat(element.style.getPropertyValue("--pointer-x")),
  );
  expect(Math.abs(x)).toBeLessThanOrEqual(8);
  await page.mouse.move(0, 0);
  await expect
    .poll(() =>
      art.evaluate((element) => element.style.getPropertyValue("--pointer-x")),
    )
    .toBe("");
  await page
    .locator("[data-story-step]")
    .nth(2)
    .evaluate((element) =>
      element.scrollIntoView({ block: "center", behavior: "instant" }),
    );
  await expect(page.locator("[data-story]")).toHaveAttribute("data-step", "2");
  await page.emulateMedia({ reducedMotion: "reduce" });
  await expect(page.locator("[data-story]")).not.toHaveAttribute("data-step");
  expect(
    await page
      .locator(".story-stage")
      .evaluate((element) => getComputedStyle(element).position),
  ).toBe("relative");
  await settle(page);
  expect(
    await page
      .locator(".story-stage .child-wheel")
      .evaluate((element) => getComputedStyle(element).opacity),
  ).toBe("1");
  await page.emulateMedia({ reducedMotion: "no-preference" });
  await page
    .locator("[data-story-step]")
    .nth(1)
    .evaluate((element) =>
      element.scrollIntoView({ block: "center", behavior: "instant" }),
    );
  await expect(page.locator("[data-story]")).toHaveAttribute("data-step", "1");
});

test("mobile touch and short desktop use the complete unpinned illustration", async ({
  browser,
  page,
}) => {
  const context = await browser.newContext({
    viewport: { width: 390, height: 844 },
    isMobile: true,
    hasTouch: true,
  });
  const mobile = await context.newPage();
  await mobile.goto(home);
  await mobile.locator(".story-stage").scrollIntoViewIfNeeded();
  await expect(mobile.locator("[data-story]")).not.toHaveAttribute("data-step");
  await settle(mobile);
  expect(
    await mobile
      .locator(".story-stage .child-wheel")
      .evaluate((element) => getComputedStyle(element).opacity),
  ).toBe("1");
  await expect(mobile.locator("[data-pointer-art]")).not.toHaveAttribute(
    "style",
  );
  await context.close();
  await page.setViewportSize({ width: 1440, height: 500 });
  await page.goto(home);
  await page.locator(".story-stage").scrollIntoViewIfNeeded();
  await expect(page.locator("[data-story]")).not.toHaveAttribute("data-step");
  expect(
    await page
      .locator(".story-stage")
      .evaluate((element) => getComputedStyle(element).position),
  ).toBe("relative");
});

test("blocked motion script leaves every workflow and feature readable", async ({
  browser,
}) => {
  const context = await browser.newContext({
    javaScriptEnabled: false,
    viewport: { width: 1440, height: 960 },
  });
  const page = await context.newPage();
  await page.goto(home);
  await expect(page.locator("[data-story-step] h3")).toHaveCount(4);
  for (const heading of await page.locator("main h2, main h3").all())
    await expect(heading).toBeVisible();
  await expect(
    page.getByText("Workflow illustration", { exact: false }),
  ).toBeVisible();
  expect(
    await page
      .locator(".story-stage .root-wheel")
      .evaluate((element) => getComputedStyle(element).opacity),
  ).toBe("1");
  await context.close();
});

test("motion settles and keeps the layout stable on arrival, scroll, and theme changes", async ({
  page,
}) => {
  await page.addInitScript(() => {
    (window as unknown as { shifts: number[] }).shifts = [];
    new PerformanceObserver((list) => {
      for (const entry of list.getEntries()) {
        const shift = entry as PerformanceEntry & {
          value: number;
          hadRecentInput: boolean;
        };
        if (!shift.hadRecentInput)
          (window as unknown as { shifts: number[] }).shifts.push(shift.value);
      }
    }).observe({ type: "layout-shift", buffered: true });
  });
  await page.goto(home);
  await page.evaluate(() => document.fonts.ready);
  await settle(page);
  for (const target of [
    "[data-story-step]:last-child",
    ".feature-row",
    ".control-feature",
    ".closing",
  ]) {
    await page.locator(target).first().scrollIntoViewIfNeeded();
    await settle(page);
  }
  const before = await page.locator(".closing").boundingBox();
  await page.getByLabel("Website theme").selectOption("dark");
  await settle(page);
  const after = await page.locator(".closing").boundingBox();
  expect(before?.height).toBe(after?.height);
  const cls = await page.evaluate(() =>
    (window as unknown as { shifts: number[] }).shifts.reduce(
      (sum, value) => sum + value,
      0,
    ),
  );
  expect(cls).toBeLessThan(0.05);
});

test("theme changes switch text surfaces atomically while decorative geometry may transition", async ({
  page,
}) => {
  await page.goto(home);
  await settle(page);
  const palette = () =>
    page.evaluate(() => {
      const button = document.querySelector(".button")!;
      const style = getComputedStyle(button);
      return {
        canvas: getComputedStyle(document.body).backgroundColor,
        button: style.backgroundColor,
        text: style.color,
      };
    });
  for (const theme of ["dark", "light"]) {
    await page.evaluate((theme) => {
      const select = document.querySelector<HTMLSelectElement>("#theme")!;
      select.value = theme;
      select.dispatchEvent(new Event("change"));
    }, theme);
    const immediately = await palette();
    await settle(page);
    expect(await palette()).toEqual(immediately);
  }
});
