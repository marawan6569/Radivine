// Anonymous, read-only smoke check of the exact local production artifact.
// Build with the production origin/base before running. No live files are written.
import assert from "node:assert/strict";
import { readdir, readFile } from "node:fs/promises";
import { createHash } from "node:crypto";
import { chromium, expect } from "@playwright/test";

const home = new URL(
  process.env.REVIEW_URL || "https://marawan6569.github.io/Radivine/",
);
assert(home.pathname.endsWith("/"));
assert(home.protocol === "https:" || home.hostname === "127.0.0.1");
const canonicalHome = "https://marawan6569.github.io/Radivine/";
const report = {
  checkedAt: new Date().toISOString(),
  url: home.href,
  files: [],
};
const hash = (bytes) => createHash("sha256").update(bytes).digest("hex");
async function request(path, status = 200) {
  const url = new URL(path, home);
  const response = await fetch(url, { signal: AbortSignal.timeout(30000) });
  assert.equal(response.status, status, url.href);
  assert.equal(response.url, url.href, `Unexpected redirect: ${url}`);
  return response;
}
async function walk(directory, prefix = "") {
  for (const entry of await readdir(directory, { withFileTypes: true })) {
    const file = prefix + entry.name;
    if (entry.isDirectory()) {
      await walk(`${directory}/${entry.name}`, `${file}/`);
      continue;
    }
    const route = file.replace(/index\.html$/, "");
    const response = await request(route);
    const actual = Buffer.from(await response.arrayBuffer());
    const expected = await readFile(`${directory}/${entry.name}`);
    assert.equal(hash(actual), hash(expected), `Artifact differs: ${file}`);
    report.files.push({ file, status: response.status, sha256: hash(actual) });
  }
}
await walk("dist");
const missing = await request("missing/w5-smoke-check/", 404);
assert.match(await missing.text(), /name="robots" content="noindex"/);
const sitemap = await (await request("sitemap.xml")).text();
const routes = [...sitemap.matchAll(/<loc>(.*?)<\/loc>/g)].map((m) => {
  assert(m[1].startsWith(canonicalHome), `Wrong sitemap origin/base: ${m[1]}`);
  return m[1].slice(canonicalHome.length);
});
assert.equal(routes.length, 9);

const browser = await chromium.launch();
try {
  const context = await browser.newContext({
    viewport: { width: 1440, height: 1000 },
    reducedMotion: "reduce",
  });
  const page = await context.newPage();
  const errors = [];
  page.on("pageerror", (error) => errors.push(error.message));
  page.on("requestfailed", (request) => errors.push(request.url()));
  page.on("response", (response) => {
    if (response.status() >= 400) errors.push(response.url());
  });
  for (const route of routes) {
    const url = new URL(route, home).href;
    const response = await page.goto(url);
    assert.equal(response.status(), 200);
    assert.equal(page.url(), url);
    await expect(page.locator("h1")).toBeVisible();
    await expect(page.locator('link[rel="canonical"]')).toHaveAttribute(
      "href",
      new URL(route, canonicalHome).href,
    );
    await expect(page.locator('meta[property="og:url"]')).toHaveAttribute(
      "content",
      new URL(route, canonicalHome).href,
    );
    assert(
      (
        await page.locator('meta[property="og:image"]').getAttribute("content")
      ).startsWith(canonicalHome),
    );
    await page.evaluate(() => document.fonts.ready);
    await page.locator("img").evaluateAll((images) => {
      for (const image of images) image.loading = "eager";
    });
    await expect
      .poll(() =>
        page
          .locator("img")
          .evaluateAll((images) =>
            images.every((image) => image.complete && image.naturalWidth > 0),
          ),
      )
      .toBe(true);
  }
  await page.goto(home.href);
  for (const theme of ["light", "dark", "system"]) {
    await page.emulateMedia({ colorScheme: "dark" });
    await page.getByLabel("Website theme").selectOption(theme);
    await expect(page.locator("html")).toHaveAttribute(
      "data-theme",
      theme === "system" ? "dark" : theme,
    );
    await page.reload();
    await expect(page.locator("html")).toHaveAttribute(
      "data-preference",
      theme,
    );
  }
  await page.emulateMedia({ colorScheme: "light" });
  await expect(page.locator("html")).toHaveAttribute("data-theme", "light");
  await page.getByRole("link", { name: "Try the wheel", exact: true }).click();
  await page.locator("[data-open]").click();
  await page.keyboard.press("1");
  await page.keyboard.press("1");
  await expect(page.locator("[data-outcome]")).toContainText(
    "No local file was opened",
  );
  await page.locator("[data-context]").selectOption("image");
  await page.getByLabel("Concentric Rings", { exact: true }).check();
  await page.locator("[data-open]").click();
  for (const index of [0, 0, 1])
    await page.locator(`[data-action="${index}"]`).click();
  await expect(page.locator("[data-outcome]")).toContainText(
    "WebP conversion job would be queued",
  );
  await page.locator("[data-invoke]").click();
  await page.keyboard.press("Escape");
  await expect(page.locator("[data-wheel-demo]")).toHaveAttribute(
    "data-phase",
    "cancelled",
  );
  assert.deepEqual(errors, []);
  await context.close();
  const noJS = await browser.newContext({ javaScriptEnabled: false });
  const staticPage = await noJS.newPage();
  await staticPage.goto(new URL("download/", home).href);
  await expect(staticPage.locator("h1")).toBeVisible();
  const release = JSON.parse(await readFile("src/data/release.json", "utf8"));
  assert(
    (await staticPage.locator(`a[href="${release.assets[0].url}"]`).count()) >
      0,
  );
  assert((await staticPage.locator("pre").count()) > 0);
  await noJS.close();
  report.browser = {
    engine: "Chromium",
    routes: routes.length,
    themes: ["light", "dark", "system"],
    demo: [
      "Branching keyboard preview",
      "Concentric WebP simulation",
      "cancel",
    ],
    noJSDownload: true,
    errors,
  };
  report.missingRouteStatus = 404;
  report.note =
    "Automated checks only. Native fidelity, screen-reader and manual cross-browser acceptance remain unresolved.";
  console.log(JSON.stringify(report, null, 2));
} finally {
  await browser.close();
}
