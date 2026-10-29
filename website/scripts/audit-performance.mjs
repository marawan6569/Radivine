import lighthouse from "lighthouse";
import { chromium } from "@playwright/test";
import { createServer } from "node:net";
import { mkdir, writeFile } from "node:fs/promises";
import { resolve } from "node:path";
const url = process.env.REVIEW_URL || "http://127.0.0.1:4342/Radivine/";
const output = resolve(
  process.env.REVIEW_OUTPUT || "test-results/w4-performance",
);
await mkdir(output, { recursive: true });
const summary = {
  url,
  measuredOn: new Date().toISOString(),
  lighthouse: [],
  controlled: [],
};
// Fresh browser per run: Lighthouse's default mobile simulated Slow 4G / 4x CPU.
for (let i = 0; i < 3; i++) {
  const reservation = createServer();
  await new Promise((resolve) => reservation.listen(0, "127.0.0.1", resolve));
  const port = reservation.address().port;
  await new Promise((resolve) => reservation.close(resolve));
  const browser = await chromium.launch({
    args: [`--remote-debugging-port=${port}`],
  });
  try {
    const { lhr, report } = await lighthouse(url, {
      port,
      output: "json",
      logLevel: "error",
      onlyCategories: ["performance", "accessibility", "best-practices", "seo"],
    });
    await writeFile(resolve(output, `lighthouse-${i + 1}.json`), report);
    summary.lighthouse.push({
      version: lhr.lighthouseVersion,
      settings: lhr.configSettings,
      scores: Object.fromEntries(
        Object.entries(lhr.categories).map(([k, v]) => [k, v.score * 100]),
      ),
      lcp: lhr.audits["largest-contentful-paint"].numericValue,
      cls: lhr.audits["cumulative-layout-shift"].numericValue,
      tbt: lhr.audits["total-blocking-time"].numericValue,
      transfer: lhr.audits["total-byte-weight"].numericValue,
      failed: Object.values(lhr.audits)
        .filter((a) => a.score !== null && a.score < 1)
        .map((a) => ({
          id: a.id,
          title: a.title,
          score: a.score,
          displayValue: a.displayValue,
        })),
    });
    console.log(
      `Lighthouse ${i + 1}: ${JSON.stringify(summary.lighthouse.at(-1).scores)}`,
    );
  } finally {
    await browser.close();
  }
}
for (const [name, viewport] of [
  ["desktop", { width: 1440, height: 1000 }],
  ["mobile", { width: 390, height: 844 }],
]) {
  const browser = await chromium.launch();
  const context = await browser.newContext({
    viewport,
    isMobile: name === "mobile",
    hasTouch: name === "mobile",
  });
  const page = await context.newPage();
  const cdp = await context.newCDPSession(page);
  await cdp.send("Network.enable");
  await cdp.send("Network.emulateNetworkConditions", {
    offline: false,
    latency: 150,
    downloadThroughput: 1_600_000 / 8,
    uploadThroughput: 750_000 / 8,
  });
  await cdp.send("Emulation.setCPUThrottlingRate", { rate: 4 });
  await page.addInitScript(() => {
    window.qaMetrics = { lcp: 0, cls: 0, shifts: [] };
    new PerformanceObserver((list) => {
      for (const e of list.getEntries()) window.qaMetrics.lcp = e.startTime;
    }).observe({ type: "largest-contentful-paint", buffered: true });
    new PerformanceObserver((list) => {
      for (const e of list.getEntries())
        if (!e.hadRecentInput) {
          window.qaMetrics.cls += e.value;
          window.qaMetrics.shifts.push({
            value: e.value,
            nodes: e.sources.map((s) => s.node?.className?.toString()),
          });
        }
    }).observe({ type: "layout-shift", buffered: true });
  });
  await page.goto(url);
  await page.evaluate(() => document.fonts.ready);
  await page.waitForTimeout(1800);
  const initial = await page.evaluate(() => ({
    ...window.qaMetrics,
    transfer:
      performance.getEntriesByType("navigation")[0].transferSize +
      performance
        .getEntriesByType("resource")
        .reduce((s, e) => s + e.transferSize, 0),
    resources: performance.getEntriesByType("resource").map((e) => ({
      name: e.name.split("/").at(-1),
      transfer: e.transferSize,
      encoded: e.encodedBodySize,
    })),
  }));
  await page.locator("#try").scrollIntoViewIfNeeded();
  await page.locator("[data-invoke]").click();
  const timing = await page.locator("[data-stage]").evaluate(async (stage) => {
    const dispatch = [],
      frames = [];
    for (let i = 0; i < 80; i++) {
      const start = performance.now();
      stage.dispatchEvent(
        new KeyboardEvent("keydown", {
          key: i % 2 ? "ArrowDown" : "ArrowUp",
          bubbles: true,
        }),
      );
      dispatch.push(performance.now() - start);
      await new Promise((resolve) =>
        requestAnimationFrame(() =>
          requestAnimationFrame(() => {
            frames.push(performance.now() - start);
            resolve();
          }),
        ),
      );
    }
    const stats = (a) => {
      a.sort((a, b) => a - b);
      return {
        median: a[Math.floor(a.length * 0.5)],
        p95: a[Math.floor(a.length * 0.95)],
        max: Math.max(...a),
      };
    };
    return {
      dispatchMs: stats(dispatch),
      twoFrameMs: stats(frames),
      note: "Synthetic key dispatch and two animation frames, not field INP or a paint guarantee.",
    };
  });
  await page.keyboard.press("Escape");
  await page.waitForTimeout(600);
  const end = await page.evaluate(() => ({
    ...window.qaMetrics,
    runningAnimations: document
      .querySelector("#try")
      .getAnimations({ subtree: true })
      .filter((a) => a.playState === "running").length,
  }));
  summary.controlled.push({
    name,
    viewport,
    cpuSlowdown: 4,
    latencyMs: 150,
    downloadBitsPerSecond: 1600000,
    initial,
    timing,
    end,
  });
  await browser.close();
}
summary.medianPerformance = summary.lighthouse
  .map((r) => r.scores.performance)
  .sort((a, b) => a - b)[1];
await writeFile(
  resolve(output, "performance.json"),
  JSON.stringify(summary, null, 2) + "\n",
);
console.log(
  JSON.stringify(
    {
      medianPerformance: summary.medianPerformance,
      controlled: summary.controlled.map((r) => ({
        name: r.name,
        lcp: r.initial.lcp,
        cls: r.end.cls,
        transfer: r.initial.transfer,
        ...r.timing,
      })),
    },
    null,
    2,
  ),
);
