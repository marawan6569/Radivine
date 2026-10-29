import { test, expect } from "@playwright/test";
const base = process.env.SITE_BASE || "/Radivine/";

test("external file and URL drops are rejected only inside the practice region", async ({
  page,
}) => {
  await page.goto(base + "#try");
  await expect(page.locator("[data-open]")).toBeVisible();
  const outcomes = await page.evaluate(() => {
    const result = [];
    for (const target of [
      document.querySelector("[data-stage]")!,
      document.querySelector("h1")!,
    ]) {
      for (const type of ["dragover", "drop"]) {
        const data = new DataTransfer();
        data.items.add(
          new File(["disposable QA fixture"], "example.txt", {
            type: "text/plain",
          }),
        );
        data.setData("text/uri-list", "https://example.org/");
        const event = new DragEvent(type, {
          bubbles: true,
          cancelable: true,
          dataTransfer: data,
        });
        target.dispatchEvent(event);
        result.push(event.defaultPrevented);
      }
    }
    return result;
  });
  expect(outcomes).toEqual([true, true, false, false]);
  await expect(page.locator("[data-wheel-demo]")).toHaveAttribute(
    "data-phase",
    "idle",
  );
  await expect(page).toHaveURL(new RegExp(`${base}#try$`));
});

test("persisted page lifecycle restores controls, model and announcements together", async ({
  page,
}) => {
  await page.goto(base + "#try");
  await page.locator("[data-context]").selectOption("image");
  await page.getByLabel("Concentric Rings", { exact: true }).check();
  await page.locator("[data-open]").click();
  await page.keyboard.press("1");
  // Deterministic persisted lifecycle; real browser back is covered separately.
  await page.evaluate(() => {
    window.dispatchEvent(
      new PageTransitionEvent("pagehide", { persisted: true }),
    );
    window.dispatchEvent(
      new PageTransitionEvent("pageshow", { persisted: true }),
    );
  });
  await expect(page.locator("[data-context]")).toHaveValue("text");
  await expect(
    page.locator('[name="wheel-layout"][value="branching"]'),
  ).toBeChecked();
  await expect(page.locator("[data-environment]")).toHaveValue("desktop");
  await expect(page.locator("[data-key-status]")).toBeEmpty();
  await page.locator("[data-open]").click();
  await page.keyboard.press("1");
  await page.keyboard.press("2");
  await expect(page.locator("[data-outcome]")).toContainText("Head");
});

const routes = [
  "",
  "download/",
  "compatibility/",
  "guide/",
  "guide/configuration/",
  "guide/troubleshooting/",
  "roadmap/",
  "contribute/",
  "licenses/",
  "404.html",
];
for (const width of [320, 640, 768, 1024, 844]) {
  test(`all routes reflow at ${width}px including 400%/200% viewport equivalents`, async ({
    page,
  }) => {
    await page.setViewportSize({ width, height: width === 844 ? 390 : 900 });
    await page.emulateMedia({ reducedMotion: "reduce" });
    for (const route of routes) {
      await page.goto(base + route);
      await page.evaluate(() => document.fonts.ready);
      expect(
        await page.evaluate(
          () => document.documentElement.scrollWidth <= innerWidth,
        ),
        route,
      ).toBe(true);
      await expect(page.locator("h1")).toBeVisible();
      await page.keyboard.press("Tab");
      await expect(
        page.getByRole("link", { name: "Skip to content" }),
      ).toBeFocused();
      await page.keyboard.press("Enter");
      await expect(page.locator("main")).toBeFocused();
    }
    await page.goto(base + "#try");
    await page.locator("[data-invoke]").click();
    await page.locator('[data-action="0"]').click();
    await page.locator('[data-action="2"]').click();
    await expect(page.locator("[data-outcome]")).toContainText("Tail");
    expect(
      await page.evaluate(
        () => document.documentElement.scrollWidth <= innerWidth,
      ),
    ).toBe(true);
  });
}

test("unknown deep routes return static 404 with usable absolute base links", async ({
  page,
}) => {
  const response = await page.goto(base + "guide/not-a-page/deeper/");
  expect(response!.status()).toBe(404);
  await expect(page.locator('meta[name="robots"]')).toHaveAttribute(
    "content",
    "noindex",
  );
  await page.getByRole("link", { name: "Back to Radivine" }).click();
  await expect(page).toHaveURL(new RegExp(`${base}$`));
});

test("real clipboard API copies on Chromium with an explicit permission grant", async ({
  page,
  context,
  browserName,
}) => {
  test.skip(
    browserName !== "chromium",
    "Other engines do not expose this permission override; adapter success/denial tested on all engines.",
  );
  await context.grantPermissions(["clipboard-read", "clipboard-write"]);
  await page.goto(base + "download/");
  await page
    .getByRole("button", { name: "Copy Install with apt", exact: true })
    .click();
  expect(await page.evaluate(() => navigator.clipboard.readText())).toBe(
    "sudo apt install ./radivine_0.3.0_amd64.deb",
  );
});

test("long practice session releases transient nodes and persisted handlers", async ({
  page,
}) => {
  await page.goto(base + "#try");
  for (let i = 0; i < 25; i++) {
    await page.locator("[data-invoke]").click();
    await page.keyboard.press("1");
    await page.keyboard.press("Backspace");
    await page.keyboard.press("Escape");
  }
  await page.waitForTimeout(400);
  expect(await page.locator("[data-ghost]").count()).toBe(0);
  expect(await page.locator("[data-wheel]").count()).toBe(1);
  expect(
    await page
      .locator("[data-wheel-demo]")
      .evaluate(
        (el) =>
          el
            .getAnimations({ subtree: true })
            .filter((a) => a.playState === "running").length,
      ),
  ).toBe(0);
  for (let i = 0; i < 5; i++)
    await page.evaluate(() => {
      window.dispatchEvent(
        new PageTransitionEvent("pagehide", { persisted: true }),
      );
      window.dispatchEvent(
        new PageTransitionEvent("pageshow", { persisted: true }),
      );
    });
  await page.locator("[data-open]").click();
  await page.keyboard.press("ArrowDown");
  await expect(page.locator("[data-key-status]")).toContainText("File Preview");
  await page.keyboard.press("Enter");
  await page.keyboard.press("2");
  await expect(page.locator("[data-outcome]")).toContainText("Head");
});
