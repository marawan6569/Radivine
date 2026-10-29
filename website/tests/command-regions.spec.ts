import { test, expect } from "@playwright/test";
const base = process.env.SITE_BASE || "/Radivine/";

test("command blocks expose named keyboard-scrollable regions", async ({
  page,
}) => {
  await page.setViewportSize({ width: 320, height: 800 });
  await page.goto(base + "download/");
  const region = page.getByRole("region", {
    name: "Download release files",
    exact: true,
  });
  await expect(region).toContainText(
    "curl -fLO https://github.com/marawan6569/Radivine/releases/download/v0.3.0/",
  );
  await region.focus();
  await expect(region).toBeFocused();
  await page.keyboard.press("ArrowRight");
  await expect
    .poll(() => region.evaluate((el) => el.scrollLeft))
    .toBeGreaterThan(0);
  await page.keyboard.press("Tab");
  await expect(region).not.toBeFocused();
});
