import { chromium } from "@playwright/test";
import { mkdir, writeFile } from "node:fs/promises";
import { resolve, join } from "node:path";

const base = process.env.REVIEW_URL || "http://127.0.0.1:4332/Radivine/";
const output = resolve(
  process.env.REVIEW_OUTPUT || "../docs/test-reports/media/W2",
);
await mkdir(output, { recursive: true });
const browser = await chromium.launch();
const runs = [];
try {
  for (const device of ["desktop", "mobile"]) {
    for (const theme of ["light", "dark"]) {
      const viewport =
        device === "desktop"
          ? { width: 1440, height: 960 }
          : { width: 390, height: 844 };
      const context = await browser.newContext({
        viewport,
        colorScheme: theme,
        reducedMotion: "reduce",
      });
      for (const route of [
        "",
        "download/",
        "compatibility/",
        "guide/configuration/",
        "roadmap/",
      ]) {
        const page = await context.newPage();
        const errors = [];
        page.on("pageerror", (error) => errors.push(error.message));
        page.on("response", (response) => {
          if (response.status() >= 400)
            errors.push(`${response.status()} ${response.url()}`);
        });
        await page.addInitScript(() => {
          window.reviewCLS = 0;
          new PerformanceObserver((list) => {
            for (const entry of list.getEntries())
              if (!entry.hadRecentInput) window.reviewCLS += entry.value;
          }).observe({ type: "layout-shift", buffered: true });
        });
        await page.goto(new URL(route, base).href);
        await page.evaluate(() => document.fonts.ready);
        await page
          .locator("img")
          .evaluateAll((images) =>
            images.forEach((image) => (image.loading = "eager")),
          );
        await page.waitForTimeout(300);
        const name = `${device}-${theme}-${route.replaceAll("/", "-").replace(/-$/, "") || "home"}`;
        await page.screenshot({
          path: join(output, `${name}.png`),
          fullPage: true,
        });
        runs.push({
          device,
          theme,
          route: route || "/",
          viewport,
          screenshot: `${name}.png`,
          ...(await page.evaluate(() => ({
            cls: window.reviewCLS,
            documentWidth: document.documentElement.scrollWidth,
            viewportWidth: innerWidth,
            resourceRequests: performance.getEntriesByType("resource").length,
          }))),
          errors,
        });
        await page.close();
      }
      await context.close();
    }
  }
  await writeFile(
    join(output, "measurements.json"),
    JSON.stringify(
      {
        note: "Local unthrottled Chromium production preview. Full-page PNGs use reduced motion for complete static overviews. These are website captures, not native Linux application tests or field performance data.",
        runs,
      },
      null,
      2,
    ) + "\n",
  );
  console.log(
    `Captured ${runs.length} full-page screenshots; ${runs.flatMap((run) => run.errors).length} browser errors.`,
  );
} finally {
  await browser.close();
}
