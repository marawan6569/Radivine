import { chromium } from "@playwright/test";
import AxeBuilder from "@axe-core/playwright";
import { mkdir, writeFile } from "node:fs/promises";
import { resolve } from "node:path";
const base = process.env.REVIEW_URL || "http://127.0.0.1:4345/Radivine/";
const output = resolve(
  process.env.REVIEW_OUTPUT || "test-results/w4-accessibility",
);
await mkdir(output, { recursive: true });
const browser = await chromium.launch();
const results = [],
  contrast = [];
const tags = [
  "wcag2a",
  "wcag2aa",
  "wcag21a",
  "wcag21aa",
  "wcag22aa",
  "best-practice",
];
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
for (const theme of ["light", "dark"]) {
  const context = await browser.newContext({
    colorScheme: theme,
    reducedMotion: "reduce",
    viewport: { width: 1440, height: 1000 },
  });
  const page = await context.newPage();
  await page.addInitScript(() => {
    const original = window.setTimeout.bind(window);
    window.setTimeout = (callback, delay, ...args) =>
      original(
        callback,
        delay > 1700 && delay <= 1800 ? 60000 : delay,
        ...args,
      );
  });
  async function audit(label, region) {
    let builder = new AxeBuilder({ page }).withTags(tags);
    if (region) builder = builder.include(region);
    const r = await builder.analyze();
    results.push({
      theme,
      label,
      violations: r.violations.map(({ id, impact, description, nodes }) => ({
        id,
        impact,
        description,
        nodes: nodes.map((n) => ({
          target: n.target,
          summary: n.failureSummary,
        })),
      })),
      incomplete: r.incomplete.map(({ id, nodes }) => ({
        id,
        targets: nodes.map((n) => n.target),
      })),
      passes: r.passes.length,
    });
  }
  for (const route of routes) {
    await page.goto(base + route);
    await page.evaluate(() => document.fonts.ready);
    await audit(route || "home");
  }
  await page.goto(base + "#try");
  await page.locator("[data-open]").waitFor({ state: "visible" });
  contrast.push(
    await page.evaluate((theme) => {
      const s = getComputedStyle(document.documentElement);
      const lum = (v) => {
        let hex = v.trim().replace("#", "");
        if (hex.length === 3) hex = [...hex].map((c) => c + c).join("");
        const rgb = hex
          .match(/../g)
          .map((v) => parseInt(v, 16) / 255)
          .map((v) =>
            v <= 0.04045 ? v / 12.92 : ((v + 0.055) / 1.055) ** 2.4,
          );
        return rgb[0] * 0.2126 + rgb[1] * 0.7152 + rgb[2] * 0.0722;
      };
      const pairs = [];
      for (const background of ["--canvas", "--surface", "--surface-muted"])
        for (const foreground of [
          "--text",
          "--text-muted",
          "--accent",
          "--focus",
        ]) {
          const a = lum(s.getPropertyValue(foreground)),
            b = lum(s.getPropertyValue(background));
          pairs.push({
            foreground,
            background,
            ratio: (Math.max(a, b) + 0.05) / (Math.min(a, b) + 0.05),
          });
        }
      const a = lum(s.getPropertyValue("--action")),
        b = lum(s.getPropertyValue("--on-action"));
      pairs.push({
        foreground: "--on-action",
        background: "--action",
        ratio: (Math.max(a, b) + 0.05) / (Math.min(a, b) + 0.05),
      });
      return { theme, pairs };
    }, theme),
  );
  for (const layout of ["branching", "concentric"]) {
    await page.locator(`input[value="${layout}"]`).check();
    for (const contextId of ["text", "multiple", "checking", "image"]) {
      await page.locator("[data-context]").selectOption(contextId);
      await page.locator("[data-invoke]").click();
      await page.locator('[data-action="0"]').click();
      if (contextId === "image")
        await page.locator('[data-action="0"]').click();
      // The init script extends only the illustrative inspection delay. Axe's
      // timers and animation frames remain live while the pending state is audited.
      if (
        contextId === "checking" &&
        !(await page.locator('[data-action="0"]').textContent()).includes(
          "Checking",
        )
      )
        throw new Error("Pending audit missed its state");
      await audit(`${layout}/${contextId}/nested`, "#try");

      if (contextId === "image") {
        await page.locator('[data-action="1"]').click();
        await audit(`${layout}/result`, "#try");
      }
    }
  }
  await page.setViewportSize({ width: 320, height: 800 });
  await page.goto(base + "compatibility/");
  await page.screenshot({
    path: resolve(output, `400-percent-reflow-${theme}.png`),
    fullPage: true,
  });
  await page.emulateMedia({ forcedColors: "active" });
  await page.goto(base + "#try");
  await page.locator("[data-invoke]").click();
  await page.keyboard.press("1");
  await page.screenshot({
    path: resolve(output, `forced-colors-${theme}.png`),
    fullPage: false,
  });
  await context.close();
}
await browser.close();
await writeFile(
  resolve(output, "accessibility.json"),
  JSON.stringify(
    {
      checkedOn: new Date().toISOString(),
      engine: "Chromium",
      tags,
      results,
      contrast,
      note: "Automated checks, reflow proxies and browser-emulated forced colors. Not screen-reader, real zoom, physical touch or WCAG certification.",
    },
    null,
    2,
  ) + "\n",
);
console.log({
  audits: results.length,
  violations: results.flatMap((r) => r.violations),
  incomplete: results
    .filter((r) => r.incomplete.length)
    .map((r) => ({
      theme: r.theme,
      label: r.label,
      rules: r.incomplete.map((i) => i.id),
    })),
  minTextContrast: Math.min(
    ...contrast.flatMap((c) =>
      c.pairs.filter((p) => p.foreground !== "--focus").map((p) => p.ratio),
    ),
  ),
  minFocusContrast: Math.min(
    ...contrast.flatMap((c) =>
      c.pairs.filter((p) => p.foreground === "--focus").map((p) => p.ratio),
    ),
  ),
});
