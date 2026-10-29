import { chromium } from "@playwright/test";
import { mkdir, writeFile, readFile, readdir, rm } from "node:fs/promises";
import { resolve, join } from "node:path";
import { gzipSync } from "node:zlib";

// Capture an already-built, locally served production artifact.
const url = process.env.REVIEW_URL || "http://127.0.0.1:4332/Radivine/";
const output = resolve(
  process.env.REVIEW_OUTPUT || "../docs/test-reports/media/W1.5",
);
await mkdir(output, { recursive: true });
const browser = await chromium.launch();
const metrics = [];
const travel = async (page, selector, duration = 1100) => {
  const destination = await page
    .locator(selector)
    .first()
    .evaluate((element) => {
      const rect = element.getBoundingClientRect();
      return Math.max(
        0,
        window.scrollY + rect.top + rect.height / 2 - innerHeight / 2,
      );
    });
  // This interpolation belongs only to the recording harness. The site never
  // intercepts scroll or runs a perpetual animation loop.
  await page.evaluate(
    async ({ destination, duration }) => {
      const start = scrollY;
      const time = performance.now();
      await new Promise((done) => {
        const move = (now) => {
          const progress = Math.min(1, (now - time) / duration);
          const eased = progress * progress * (3 - 2 * progress);
          window.scrollTo({
            top: start + (destination - start) * eased,
            behavior: "instant",
          });
          if (progress < 1) requestAnimationFrame(move);
          else done();
        };
        requestAnimationFrame(move);
      });
    },
    { destination, duration },
  );
  await page.waitForTimeout(900);
};
try {
  for (const [device, theme] of [
    ["desktop", "light"],
    ["desktop", "dark"],
    ["mobile", "light"],
    ["mobile", "dark"],
  ]) {
    const viewport =
      device === "desktop"
        ? { width: 1440, height: 960 }
        : { width: 390, height: 844 };
    const context = await browser.newContext({
      viewport,
      colorScheme: theme,
      reducedMotion: "no-preference",
      isMobile: device === "mobile",
      hasTouch: device === "mobile",
      recordVideo: {
        dir: join(output, ".recordings"),
        size: device === "desktop" ? { width: 960, height: 640 } : viewport,
      },
    });
    const page = await context.newPage();
    const errors = [];
    page.on("pageerror", (error) => errors.push(error.message));
    page.on("response", (response) => {
      if (response.status() >= 400)
        errors.push(`${response.status()} ${response.url()}`);
    });
    await page.addInitScript(() => {
      window.reviewMetrics = { cls: 0, lcp: 0, longTasks: [] };
      new PerformanceObserver((list) => {
        for (const entry of list.getEntries())
          if (!entry.hadRecentInput) window.reviewMetrics.cls += entry.value;
      }).observe({ type: "layout-shift", buffered: true });
      new PerformanceObserver((list) => {
        for (const entry of list.getEntries())
          window.reviewMetrics.lcp = entry.startTime;
      }).observe({ type: "largest-contentful-paint", buffered: true });
      new PerformanceObserver((list) => {
        for (const entry of list.getEntries())
          window.reviewMetrics.longTasks.push(Math.round(entry.duration));
      }).observe({ type: "longtask", buffered: true });
    });
    await page.goto(url);
    await page.evaluate(() => document.fonts.ready);
    await page.waitForTimeout(1800);
    await page.screenshot({
      path: join(output, `${device}-${theme}-hero.png`),
    });
    if (device === "desktop") {
      await page.mouse.move(1000, 320);
      await page.waitForTimeout(450);
      await page.mouse.move(1200, 500, { steps: 12 });
      await page.waitForTimeout(450);
      await page.mouse.move(0, 0);
      for (let step = 1; step <= 4; step++) {
        await travel(page, `[data-story-step]:nth-child(${step})`);
        if (step === 3)
          await page.screenshot({
            path: join(output, `${device}-${theme}-branch.png`),
          });
      }
    } else {
      await travel(page, ".hero-stage");
      await travel(page, ".story-stage");
      await travel(page, "[data-story-step]:nth-child(3)");
    }
    await travel(page, ".feature-row:first-of-type", 1600);
    await travel(page, ".feature-reverse", 1200);
    await page.waitForTimeout(500);
    const measurement = await page.evaluate(() => ({
      ...window.reviewMetrics,
      requests: performance.getEntriesByType("resource").length,
      transferBytes: performance
        .getEntriesByType("resource")
        .reduce((sum, entry) => sum + entry.transferSize, 0),
      domNodes: document.querySelectorAll("*").length,
    }));
    metrics.push({ device, theme, viewport, ...measurement, errors });
    const video = page.video();
    await context.close();
    await video.saveAs(join(output, `${device}-${theme}-motion.webm`));
    await video.delete();
    console.log(
      `Recorded ${device} ${theme}: CLS ${measurement.cls.toFixed(5)}, LCP ${Math.round(measurement.lcp)}ms, ${errors.length} errors`,
    );
    // Reduced-motion overviews intentionally show the compact story and the
    // complete illustration together. Normal motion is captured above.
    const stillContext = await browser.newContext({
      viewport,
      colorScheme: theme,
      reducedMotion: "reduce",
    });
    const still = await stillContext.newPage();
    await still.goto(url);
    await still.evaluate(() => document.fonts.ready);
    await still
      .locator("img")
      .evaluateAll((images) =>
        images.forEach((image) => (image.loading = "eager")),
      );
    await still.screenshot({
      path: join(output, `${device}-${theme}-full.png`),
      fullPage: true,
    });
    await stillContext.close();
  }
  const html = await readFile("dist/index.html", "utf8");
  const scripts = [...html.matchAll(/<script\b[^>]*>([\s\S]*?)<\/script>/g)]
    .map((match) => match[1])
    .join("\n");
  const assets = await readdir("dist/_astro");
  const css = await Promise.all(
    assets
      .filter((file) => file.endsWith(".css"))
      .map((file) => readFile(join("dist/_astro", file))),
  );
  await writeFile(
    join(output, "measurements.json"),
    JSON.stringify(
      {
        note: "Local Chromium, unthrottled production build with video capture; not field data or a Lighthouse score. Full-page PNGs use reduced motion; recordings use normal motion.",
        inlineJavaScript: {
          rawBytes: Buffer.byteLength(scripts),
          gzipBytes: gzipSync(scripts).length,
        },
        css: {
          rawBytes: css.reduce((sum, data) => sum + data.length, 0),
          gzipBytes: css.reduce((sum, data) => sum + gzipSync(data).length, 0),
        },
        runs: metrics,
      },
      null,
      2,
    ) + "\n",
  );
  await rm(join(output, ".recordings"), { recursive: true, force: true });
} finally {
  await browser.close();
}
