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
