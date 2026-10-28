import { chromium, expect } from "@playwright/test";
import { mkdir, writeFile, readdir, readFile, rm } from "node:fs/promises";
import { resolve, join } from "node:path";
import { gzipSync } from "node:zlib";
const url = process.env.REVIEW_URL || "http://127.0.0.1:4332/Radivine/";
const output = resolve(
  process.env.REVIEW_OUTPUT || "../docs/test-reports/media/W3",
);
await mkdir(output, { recursive: true });
const browser = await chromium.launch();
const measurements = [];
const settle = (page) => page.waitForTimeout(450);
async function still(page, name) {
  const phase = name.includes("outcome") ? "result" : "active";
  await expect(page.locator("#try")).toHaveAttribute("data-phase", phase);
  if (page.viewportSize().width < 600) {
    const selector = /outcome|disabled/.test(name)
      ? ".practice-panel"
      : ".practice-visual";
    await page.locator(selector).scrollIntoViewIfNeeded();
    // Native viewport captures avoid fixed-element stitching artifacts on mobile.
    await page.screenshot({ path: join(output, name) });
  } else {
    // Element screenshots of the tall section can temporarily resize/scroll the
    // viewport and trigger the correct offscreen cancellation. Capture the real
    // viewport instead so review images preserve the interaction being labeled.
    await page
      .locator(".practice-workspace")
      .evaluate((el) =>
        el.scrollIntoView({ block: "start", behavior: "instant" }),
      );
    await page.screenshot({ path: join(output, name) });
  }
  await expect(page.locator("#try")).toHaveAttribute("data-phase", phase);
}

try {
  for (const device of ["desktop", "mobile"])
    for (const theme of ["light", "dark"]) {
      const viewport =
        device === "desktop"
          ? { width: 1440, height: 1000 }
          : { width: 390, height: 844 };
      const context = await browser.newContext({
        viewport,
        colorScheme: theme,
        hasTouch: device === "mobile",
        isMobile: device === "mobile",
        recordVideo: {
          dir: join(output, ".recordings"),
          size: device === "desktop" ? { width: 1152, height: 800 } : viewport,
        },
      });
      const page = await context.newPage(),
        errors = [],
        requests = [];
      page.on("pageerror", (error) => errors.push(error.message));
      page.on("response", (response) => {
        if (response.status() >= 400)
          errors.push(`${response.status()} ${response.url()}`);
      });
      page.on("request", (request) => requests.push(request.url()));
      await page.addInitScript(() => {
        window.demoMetrics = { cls: 0, longTasks: [] };
        new PerformanceObserver((list) => {
          for (const e of list.getEntries())
            if (!e.hadRecentInput) window.demoMetrics.cls += e.value;
        }).observe({ type: "layout-shift", buffered: true });
        new PerformanceObserver((list) => {
          for (const e of list.getEntries())
            window.demoMetrics.longTasks.push(Math.round(e.duration));
        }).observe({ type: "longtask", buffered: true });
      });
      await page.goto(url);
      await page.evaluate(() => document.fonts.ready);
      await page.waitForTimeout(1300);
      const initial = await page.evaluate(() => ({
        requests: performance.getEntriesByType("resource").map((e) => e.name),
        transferBytes: performance
          .getEntriesByType("resource")
          .reduce((sum, e) => sum + e.transferSize, 0),
      }));
      await page
        .getByRole("link", { name: "Try the wheel", exact: true })
        .click();
      await settle(page);
      await page.locator("[data-open]").click();
      const surface = page.locator("[data-stage]");
      await page.locator('[data-action="0"]').click();
      await settle(page);
      await still(page, `${device}-${theme}-branching.png`);
      if (device === "desktop") {
        await surface.scrollIntoViewIfNeeded();
        await surface.focus();
        await page.keyboard.press("ArrowRight");
        await settle(page);
        await page.keyboard.press("Enter");
      } else await page.locator('[data-action="1"]').tap();
      await page.locator("[data-outcome]").scrollIntoViewIfNeeded();
      await settle(page);
      await page.locator("[data-context]").selectOption("image");
      await page.getByLabel("Concentric Rings", { exact: true }).check();
      await page.locator("[data-open]").click();
      await page.locator('[data-action="0"]').click();
      await settle(page);
      await page.locator('[data-action="0"]').click();
      await settle(page);
      await still(page, `${device}-${theme}-concentric.png`);
      await page.locator('[data-action="1"]').click();
      await page.locator("[data-outcome]").scrollIntoViewIfNeeded();
      await settle(page);
      await still(page, `${device}-${theme}-outcome.png`);
      await page.locator("[data-context]").selectOption("multiple");
      await page.locator("[data-open]").click();
      await page.locator('[data-action="0"]').click();
      await settle(page);
      await still(page, `${device}-${theme}-disabled.png`);
      await page.locator("[data-cancel]").click();
      await settle(page);
      const observed = await page.evaluate(() => ({
        ...window.demoMetrics,
        overflow: document.documentElement.scrollWidth > innerWidth,
        domNodes: document.querySelectorAll("*").length,
        runningDemoAnimations: document
          .querySelector("#try")
          .getAnimations({ subtree: true })
          .filter((a) => a.playState === "running").length,
      }));
      measurements.push({
        device,
        theme,
        viewport,
        initial,
        ...observed,
        errors,
        externalRequests: requests.filter(
          (u) => !u.startsWith(new URL(url).origin),
        ),
      });
      const video = page.video();
      await context.close();
      await video.saveAs(join(output, `${device}-${theme}-practice.webm`));
      await video.delete();
      console.log(
        `Captured ${device}/${theme}: CLS ${observed.cls.toFixed(5)}, errors ${errors.length}`,
      );
    }
  // Synchronous event-dispatch cost under CPU throttling: local lab proxy, not INP.
  const context = await browser.newContext({
    viewport: { width: 1440, height: 1000 },
    reducedMotion: "reduce",
  });
  const page = await context.newPage();
  await page.goto(`${url}#try`);
  await page.locator("[data-open]").click();
  await page.locator("[data-stage]").scrollIntoViewIfNeeded();
  const client = await context.newCDPSession(page);
  await client.send("Emulation.setCPUThrottlingRate", { rate: 4 });
  const input = await page.evaluate(() => {
    const stage = document.querySelector("[data-stage]"),
      svg = document.querySelector("[data-wheel-svg]"),
      m = svg.getScreenCTM(),
      times = [];
    for (let i = 0; i < 200; i++) {
      const p = new DOMPoint(
        530 + Math.sin(i) * 12,
        350 + Math.cos(i) * 12,
      ).matrixTransform(m);
      const start = performance.now();
      stage.dispatchEvent(
        new PointerEvent("pointermove", {
          pointerId: 1,
          isPrimary: true,
          pointerType: "mouse",
          clientX: p.x,
          clientY: p.y,
          bubbles: true,
        }),
      );
      times.push(performance.now() - start);
    }
    times.sort((a, b) => a - b);
    return {
      samples: times.length,
      medianMs: times[100],
      p95Ms: times[190],
      maxMs: times.at(-1),
      cpuThrottle: 4,
    };
  });
  await context.close();
  const bundles = [];
  for (const name of await readdir("dist/_astro"))
    if (name.endsWith(".js")) {
      const bytes = await readFile(join("dist/_astro", name));
      bundles.push({
        name,
        rawBytes: bytes.length,
        gzipBytes: gzipSync(bytes).length,
      });
    }
  await writeFile(
    join(output, "measurements.json"),
    JSON.stringify(
      {
        method:
          "Local Chromium production build; videos enabled for captures. Input cost measures synchronous JS/DOM work, excludes paint; not field INP/LCP or native performance.",
        measurements,
        input,
        bundles,
      },
      null,
      2,
    ) + "\n",
  );
} finally {
  await browser.close();
  await rm(join(output, ".recordings"), { recursive: true, force: true });
}
