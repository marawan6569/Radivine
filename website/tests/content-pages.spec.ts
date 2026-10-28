import { test, expect } from "@playwright/test";
import AxeBuilder from "@axe-core/playwright";
import { readFileSync } from "node:fs";
const release = JSON.parse(
  readFileSync(new URL("../src/data/release.json", import.meta.url), "utf8"),
);
const base = process.env.SITE_BASE || "/Radivine/";
const routes = [
  "download/",
  "compatibility/",
  "guide/",
  "guide/configuration/",
  "guide/troubleshooting/",
  "roadmap/",
  "contribute/",
  "licenses/",
];
for (const theme of ["light", "dark"] as const)
  for (const width of [390, 1440])
    for (const route of routes) {
      test(`${route} ${theme} ${width}: semantics, assets, responsive, SEO`, async ({
        page,
      }) => {
        await page.setViewportSize({ width, height: 960 });
        await page.emulateMedia({
          colorScheme: theme,
          reducedMotion: "reduce",
        });
        const errors: string[] = [];
        page.on("pageerror", (e) => errors.push(e.message));
        page.on("response", (r) => {
          if (r.status() >= 400) errors.push(r.url());
        });
        await page.goto(base + route);
        await page.evaluate(() => document.fonts.ready);
        await expect(page.locator("h1")).toHaveCount(1);
        await expect(page.locator("h1")).not.toBeEmpty();
        await expect(page.locator('link[rel="canonical"]')).toHaveAttribute(
          "href",
          `https://marawan6569.github.io${base}${route}`,
        );
        expect(await page.title()).toMatch(/— Radivine$/);
        expect(
          await page
            .locator('meta[name="description"]')
            .getAttribute("content"),
        ).toBeTruthy();
        expect(
          await page.evaluate(
            () => document.documentElement.scrollWidth <= innerWidth,
          ),
        ).toBe(true);
        expect((await new AxeBuilder({ page }).analyze()).violations).toEqual(
          [],
        );
        expect(errors).toEqual([]);
      });
    }
test("download uses one release manifest, generated commands and structured data", async ({
  page,
}) => {
  await page.goto(base + "download/");
  await expect(
    page.getByRole("link", { name: `Download v${release.version}` }),
  ).toHaveAttribute("href", release.assets[0].url);
  await expect(
    page.locator("code").filter({ hasText: release.assets[0].sha256 }),
  ).toHaveText(release.assets[0].sha256);
  await expect(
    page.getByLabel("Install with apt", { exact: true }),
  ).toContainText(`sudo apt install ./${release.assets[0].name}`);
  const ld = JSON.parse(
    (await page.locator('script[type="application/ld+json"]').textContent()) ||
      "{}",
  );
  expect(ld.softwareVersion).toBe(release.version);
  expect(ld.downloadUrl).toBe(release.assets[0].url);
});
test("clipboard success and denial keep commands available", async ({
  page,
  context,
}) => {
  // Firefox/WebKit do not expose Chromium's clipboard permission override.
  // Verify the same success/denial UI with a deterministic clipboard adapter.
  await context.addInitScript(() => {
    let copied = "";
    Object.defineProperty(navigator, "clipboard", {
      value: {
        writeText: async (text: string) => {
          copied = text;
        },
        readText: async () => copied,
      },
      configurable: true,
    });
  });
  await page.goto(base + "download/");
  await page
    .getByRole("button", { name: "Copy Install with apt", exact: true })
    .click();
  expect(await page.evaluate(() => navigator.clipboard.readText())).toBe(
    `sudo apt install ./${release.assets[0].name}`,
  );
  await page.evaluate(() =>
    Object.defineProperty(navigator, "clipboard", {
      value: { writeText: () => Promise.reject(new Error("denied")) },
      configurable: true,
    }),
  );
  await page
    .getByRole("button", { name: "Copy Verify SHA-256", exact: true })
    .click();
  await expect(
    page.getByText(
      "Copy unavailable. Select and copy the command text manually.",
    ),
  ).toBeVisible();
  await expect(page.getByLabel("Verify SHA-256", { exact: true })).toHaveText(
    "sha256sum --check SHA256SUMS.txt",
  );
});
test("no-JS installation, guide disclosures and matrix retain information", async ({
  browser,
}) => {
  const context = await browser.newContext({
    javaScriptEnabled: false,
    viewport: { width: 320, height: 844 },
  });
  const page = await context.newPage();
  await page.goto(base + "download/");
  await expect(page.getByRole("button", { name: /Copy/ })).toHaveCount(0);
  await expect(
    page.getByLabel("Install with apt", { exact: true }),
  ).toBeVisible();
  await page.locator("summary").first().click();
  await page
    .getByRole("navigation", { name: "Mobile navigation" })
    .getByRole("link", { name: "Guide", exact: true })
    .click();
  await expect(page).toHaveURL(new RegExp(`${base}guide/$`));
  await page.locator("#feature-custom-actions > summary").click();
  await expect(page.locator("#feature-custom-actions")).toContainText(
    "only log",
  );
  await page.goto(base + "compatibility/");
  await expect(page.getByRole("table")).toHaveCount(1);
  await expect(page.locator('th[scope="row"]')).toHaveCount(8);
  const region = page.getByRole("region", {
    name: "Desktop compatibility matrix",
  });
  await region.focus();
  await expect(region).toBeFocused();
  await page.keyboard.press("ArrowRight");
  await expect
    .poll(() => region.evaluate((e) => e.scrollLeft))
    .toBeGreaterThan(0);
  expect(
    await page.evaluate(
      () => document.documentElement.scrollWidth <= innerWidth,
    ),
  ).toBe(true);
  for (const route of routes) {
    await page.goto(base + route);
    await expect(page.locator("h1")).toBeVisible();
    await expect(
      page.getByRole("link", { name: "Licenses & privacy", exact: true }),
    ).toBeVisible();
    expect(
      await page.evaluate(
        () => document.documentElement.scrollWidth <= innerWidth,
      ),
    ).toBe(true);
  }
  await context.close();
});
test("themes persist across nested pages and new navigation works by keyboard", async ({
  page,
}) => {
  await page.goto(base + "download/");
  await page.getByLabel("Website theme").selectOption("dark");
  await page.goto(base + "guide/configuration/");
  await expect(page.locator("html")).toHaveAttribute("data-theme", "dark");
  await page.keyboard.press("Tab");
  await expect(
    page.getByRole("link", { name: "Skip to content" }),
  ).toBeFocused();
  await page.keyboard.press("Enter");
  await expect(page.locator("#main")).toBeFocused();
  const troubleshooting = page
    .getByRole("navigation", { name: "Documentation" })
    .getByRole("link", { name: /Troubleshooting/ });
  // Keep this a keyboard path. A pointer click immediately after the skip
  // link can race Firefox's still-running native smooth anchor scroll.
  for (let tab = 0; tab < 10; tab++) {
    await page.keyboard.press("Tab");
    if (
      await troubleshooting.evaluate((link) => link === document.activeElement)
    )
      break;
  }
  await expect(troubleshooting).toBeFocused();
  await page.keyboard.press("Enter");
  await expect(page).toHaveURL(new RegExp(`${base}guide/troubleshooting/$`));

  // Independently retain real pointer navigation, without a preceding scroll.
  await page.goto(base + "guide/configuration/");
  await troubleshooting.click();
  await expect(page).toHaveURL(new RegExp(`${base}guide/troubleshooting/$`));
});
