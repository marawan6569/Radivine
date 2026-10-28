import { test, expect } from "@playwright/test";
const base = process.env.SITE_BASE || "/Radivine/";

test("lazy demo reserves its idle layout before revealing controls", async ({
  page,
}) => {
  await page.goto(base);
  await page.evaluate(() => document.fonts.ready);
  await expect(page.locator("#try")).toHaveAttribute("data-preparing", "true");
  const before = await page
    .locator(".compatibility-section")
    .evaluate((el) => (el as HTMLElement).offsetTop);
  await page.locator("#try").scrollIntoViewIfNeeded();
  await expect(page.locator("[data-open]")).toBeVisible();
  const after = await page
    .locator(".compatibility-section")
    .evaluate((el) => (el as HTMLElement).offsetTop);
  expect(Math.abs(after - before)).toBeLessThanOrEqual(1);
});
