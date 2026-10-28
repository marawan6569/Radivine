import { test, expect, type Page } from "@playwright/test";
import AxeBuilder from "@axe-core/playwright";
import { initial, geometry, reduce } from "../src/demo/model";
import { itemPoint, anchor, type Point } from "../src/demo/geometry";
const home = process.env.SITE_BASE || "/Radivine/";
const demo = (page: Page) => page.locator("[data-wheel-demo]");
const action = (page: Page, index: number) =>
  page.locator(`[data-action="${index}"]`);
async function load(page: Page) {
  await page.goto(`${home}#try`);
  await expect(page.locator("[data-open]")).toBeVisible();
}
async function open(page: Page) {
  await page.locator("[data-open]").click();
}
async function coords(page: Page, p: Point) {
  return page.locator("[data-wheel-svg]").evaluate((svg, p) => {
    const q = new DOMPoint(p.x, p.y).matrixTransform(
      (svg as SVGSVGElement).getScreenCTM()!,
    );
    return { x: q.x, y: q.y };
  }, p);
}
async function move(page: Page, p: Point) {
  const q = await coords(page, p);
  await page.mouse.move(q.x, q.y);
}
test("demo lazy-loads separately; CTA, SVG/list navigation and safe simulated result", async ({
  page,
}) => {
  await page.goto(home);
  expect(
    (
      await page.evaluate(() =>
        performance.getEntriesByType("resource").map((r) => r.name),
      )
    ).some((n) => /controller\./.test(n)),
  ).toBe(false);
  await page.getByRole("link", { name: "Try the wheel", exact: true }).click();
  await open(page);
  await action(page, 0).click();
  await expect(page.locator('[data-wheel="0"]')).toBeVisible();
  await expect(action(page, 0)).toBeFocused();
  await action(page, 0).click();
  await expect(demo(page)).toHaveAttribute("data-phase", "result");
  await expect(page.locator("[data-outcome]")).toContainText(
    "No local file was opened",
  );
  await expect(page.locator("[data-invoke]")).toBeFocused();
});
test("Concentric image path, source-format pruning and theme-independent state", async ({
  page,
}) => {
  await load(page);
  await page.locator("[data-context]").selectOption("image");
  await page.getByLabel("Concentric Rings", { exact: true }).check();
  await open(page);
  await action(page, 0).click();
  await action(page, 0).click();
  await expect(page.locator("[data-action-list]")).not.toContainText("PNG");
  await expect(page.locator('[data-wheel="0.0"]')).toBeVisible();
  for (const preference of ["dark", "light", "system"]) {
    await page.getByLabel("Website theme").evaluate((element, value) => {
      (element as HTMLSelectElement).value = value;
      element.dispatchEvent(new Event("change"));
    }, preference);
    await expect(page.locator("[data-path]")).toHaveText(
      "Files / Convert / Image",
    );
    await expect(demo(page)).toHaveAttribute("data-phase", "active");
  }
  await action(page, 1).click();
  await expect(page.locator("[data-outcome]")).toContainText(
    "WebP conversion job would be queued",
  );
});
test("keyboard arrows, digits, back, escape and Tab exit stay scoped", async ({
  page,
}) => {
  await load(page);
  await open(page);
  const surface = page.getByRole("group", { name: "Wheel practice surface" });
  await expect(surface).toBeFocused();
  await page.keyboard.press("ArrowDown");
  await page.keyboard.press("Enter");
  await expect(page.locator("[data-path]")).toContainText("File Preview");
  await page.keyboard.press("Backspace");
  await expect(page.locator("[data-path]")).toHaveText("Files");
  await page.keyboard.press("1");
  await page.keyboard.press("3");
  await expect(page.locator("[data-outcome]")).toContainText(
    "Tail (last 50 lines)",
  );
  await page.locator("[data-invoke]").click();
  await page.keyboard.press("Tab");
  await expect(surface).not.toBeFocused();
  await surface.focus();
  await page.keyboard.press("Escape");
  await expect(demo(page)).toHaveAttribute("data-phase", "cancelled");
  await page.locator("[data-reset]").focus();
  await page.keyboard.press("1");
  await expect(demo(page)).toHaveAttribute("data-phase", "cancelled");
});
test("pointer dwell opens Branching; click commits only matching leaf", async ({
  page,
}) => {
  await load(page);
  await open(page);
  let state = reduce(initial(), { type: "start" }, 0);
  await move(page, itemPoint(geometry(state)[0], 0, 70));
  await expect(page.locator('[data-wheel="0"]')).toBeVisible();
  state = reduce(state, { type: "key", key: "1" }, 110);
  await move(page, itemPoint(geometry(state)[1], 1));
  await page.mouse.down();
  await page.mouse.up();
  await expect(page.locator("[data-outcome]")).toContainText(
    "Head (first 50 lines)",
  );
});
test("synthetic drag needs movement and fresh Shift, commits on release and toggles safely", async ({
  page,
}) => {
  await load(page);
  const drag = page.locator("[data-drag]");
  await drag.hover();
  await page.mouse.down();
  await page.keyboard.press("Shift");
  await expect(demo(page)).toHaveAttribute("data-phase", "idle");
  const source = await drag.boundingBox();
  await page.mouse.move(source!.x + source!.width / 2 + 15, source!.y + 10);
  await page.keyboard.down("Shift");
  await expect(demo(page)).toHaveAttribute("data-phase", "active");
  await move(page, itemPoint(geometry(initial())[0], 1));
  await page.mouse.up();
  await page.keyboard.up("Shift");
  await expect(page.locator("[data-outcome]")).toContainText("X11 clipboard");
  await drag.hover();
  await page.mouse.down();
  const b = await drag.boundingBox();
  await page.mouse.move(b!.x + b!.width / 2 + 15, b!.y + 10);
  await page.keyboard.press("Shift");
  await expect(demo(page)).toHaveAttribute("data-phase", "active");
  await page.keyboard.press("Shift");
  await page.mouse.up();
  await expect(demo(page)).toHaveAttribute("data-phase", "cancelled");
});
test("Shift held before drag does not summon; cancelled pointer releases capture", async ({
  page,
}) => {
  await load(page);
  await page.keyboard.down("Shift");
  await page.locator("[data-drag]").hover();
  await page.mouse.down();
  await move(page, { x: 470, y: 440 });
  await expect(demo(page)).toHaveAttribute("data-phase", "idle");
  await page.keyboard.up("Shift");
  await page.keyboard.press("Shift");
  await expect(demo(page)).toHaveAttribute("data-phase", "active");
  await page
    .locator("[data-stage]")
    .dispatchEvent("pointercancel", { pointerId: 1 });
  await page.mouse.up();
  await expect(demo(page)).toHaveAttribute("data-phase", "cancelled");
  await expect(demo(page)).not.toHaveAttribute("data-dragging", "true");
});
test("neutral hub cancels; disabled and pending actions explain their state", async ({
  page,
}) => {
  await load(page);
  await open(page);
  await move(page, anchor);
  await page.mouse.down();
  await page.mouse.up();
  await expect(demo(page)).toHaveAttribute("data-phase", "cancelled");
  await page.locator("[data-context]").selectOption("multiple");
  await open(page);
  await action(page, 0).click();
  await expect(action(page, 0)).toHaveAttribute("aria-disabled", "true");
  await expect(action(page, 0)).toContainText(
    "Select a single file to preview",
  );
  await action(page, 0).focus();
  await page.keyboard.press("Enter");
  await expect(demo(page)).toHaveAttribute("data-phase", "active");
  await page.locator("[data-context]").selectOption("checking");
  await open(page);
  await action(page, 0).click();
  await expect(action(page, 0)).toContainText("Checking…");
  await expect(action(page, 0)).toHaveAttribute("aria-disabled", "true");
  await expect(action(page, 0)).toHaveAttribute("aria-disabled", "false", {
    timeout: 4000,
  });
  await action(page, 0).click();
  await expect(demo(page)).toHaveAttribute("data-phase", "result");
});
test("layout and context changes reset state; all explicit fallbacks are explained", async ({
  page,
}) => {
  await load(page);
  await open(page);
  await action(page, 0).click();
  await page.getByLabel("Concentric Rings", { exact: true }).check();
  await expect(demo(page)).toHaveAttribute("data-phase", "idle");
  await expect(page.locator('[data-wheel="0"]')).toHaveCount(0);
  await page
    .getByText("Layout fallbacks & simulation details", { exact: true })
    .click();
  for (const [value, reason] of [
    ["small", "Insufficient space"],
    ["uncomposited", "No compositor"],
    ["small-targets", "Targets too small"],
  ]) {
    await page.locator("[data-environment]").selectOption(value);
    await expect(page.locator("[data-fallback]")).toContainText(reason);
    await expect(page.locator("[data-layout-label]")).toContainText(
      "Branching",
    );
  }
});
test("offscreen session cancels inspection and never resumes an old timer", async ({
  page,
}) => {
  await load(page);
  await page.locator("[data-context]").selectOption("checking");
  await open(page);
  await page.evaluate(() => window.scrollTo({ top: 0, behavior: "instant" }));
  await expect(demo(page)).toHaveAttribute("data-phase", "cancelled");
  const status = await page.locator("[data-outcome]").textContent();
  await page.waitForTimeout(1900);
  expect(await page.locator("[data-outcome]").textContent()).toBe(status);
});
for (const scheme of ["light", "dark"] as const) {
  test(`accessibility in ${scheme}: active, nested, disabled, pending and result`, async ({
    page,
  }) => {
    await page.emulateMedia({ colorScheme: scheme, reducedMotion: "reduce" });
    await load(page);
    await open(page);
    await action(page, 0).click();
    expect(
      (await new AxeBuilder({ page }).include("#try").analyze()).violations,
    ).toEqual([]);
    await page.locator("[data-context]").selectOption("multiple");
    await open(page);
    await action(page, 0).click();
    expect(
      (await new AxeBuilder({ page }).include("#try").analyze()).violations,
    ).toEqual([]);
    await page.locator("[data-context]").selectOption("checking");
    await open(page);
    await action(page, 0).click();
    expect(
      (await new AxeBuilder({ page }).include("#try").analyze()).violations,
    ).toEqual([]);
    await page.locator("[data-context]").selectOption("image");
    await open(page);
    await action(page, 0).click();
    await action(page, 0).click();
    await action(page, 1).click();
    expect(
      (await new AxeBuilder({ page }).include("#try").analyze()).violations,
    ).toEqual([]);
  });
}
test("reduced motion changes live, settles all effects without changing navigation", async ({
  page,
}) => {
  await load(page);
  await open(page);
  await action(page, 0).click();
  await page.emulateMedia({ reducedMotion: "reduce" });
  expect(
    await demo(page).evaluate(
      (el) =>
        el
          .getAnimations({ subtree: true })
          .filter((a) => a.playState === "running").length,
    ),
  ).toBe(0);
  await action(page, 1).click();
  await expect(page.locator("[data-outcome]")).toContainText("Head");
  await page.emulateMedia({ reducedMotion: "no-preference" });
  await page.locator("[data-invoke]").click();
  await page.waitForTimeout(400);
  expect(
    await demo(page).evaluate(
      (el) =>
        el
          .getAnimations({ subtree: true })
          .filter((a) => a.playState === "running").length,
    ),
  ).toBe(0);
});
test("touch alternative, 320px layout and normal page scrolling", async ({
  browser,
}) => {
  const context = await browser.newContext({
    viewport: { width: 320, height: 740 },
    isMobile: true,
    hasTouch: true,
    reducedMotion: "reduce",
  });
  const page = await context.newPage();
  await load(page);
  await page.locator("[data-open]").tap();
  await action(page, 0).tap();
  await action(page, 2).tap();
  await expect(page.locator("[data-outcome]")).toContainText("Tail");
  expect(
    await page.evaluate(
      () => document.documentElement.scrollWidth <= innerWidth,
    ),
  ).toBe(true);
  for (const button of await page.locator("[data-action-list] button").all()) {
    const box = await button.boundingBox();
    expect(box!.height).toBeGreaterThanOrEqual(44);
  }
  expect(
    (await new AxeBuilder({ page }).include("#try").analyze()).violations,
  ).toEqual([]);
  await context.close();
});
test("no-JavaScript leaves useful paths, guide and CTA without pretend controls", async ({
  browser,
}) => {
  const context = await browser.newContext({
    javaScriptEnabled: false,
    viewport: { width: 390, height: 844 },
  });
  const page = await context.newPage();
  await page.goto(`${home}#try`);
  await expect(page.locator("[data-demo-static]")).toBeVisible();
  await expect(page.locator("[data-demo-static]")).toContainText(
    "File Preview",
  );
  await expect(page.locator("[data-open]")).not.toBeVisible();
  await expect(
    page.getByRole("link", { name: "usage guide", exact: true }),
  ).toHaveAttribute("href", `${home}guide/`);
  await context.close();
});
