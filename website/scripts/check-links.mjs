import { readFile, readdir, lstat } from "node:fs/promises";
import { resolve, join, extname } from "node:path";
import { createHash } from "node:crypto";
import assert from "node:assert/strict";

const output = resolve("dist");
const base =
  `/${(process.env.SITE_BASE || "/Radivine/").replace(/^\/+|\/+$/g, "")}`.replace(
    /\/$/,
    "",
  ) + "/";
const origin = process.env.SITE_ORIGIN || "https://marawan6569.github.io";
async function walk(directory) {
  const paths = [];
  for (const entry of await readdir(directory)) {
    const path = join(directory, entry);
    const stat = await lstat(path);
    assert(!stat.isSymbolicLink(), `Symlink in artifact: ${path}`);
    if (stat.isDirectory()) paths.push(...(await walk(path)));
    else paths.push(path);
  }
  return paths;
}
const files = await walk(output);
async function target(url) {
  assert(url.pathname.startsWith(base), `Escapes base ${base}: ${url}`);
  let path = resolve(
    output,
    decodeURIComponent(url.pathname.slice(base.length)) || "index.html",
  );
  assert(
    path.startsWith(output + "/") || path === output,
    `Escapes output: ${url}`,
  );
  const stat = await lstat(path);
  if (stat.isDirectory()) path = join(path, "index.html");
  await lstat(path);
  return path;
}
let references = 0;
for (const file of files.filter((path) =>
  [".html", ".css", ".xml"].includes(extname(path)),
)) {
  const content = await readFile(file, "utf8");
  assert(
    !/\/home\/mfazora|file:\/\/|sourceMappingURL/.test(content),
    `Unwanted local/source path in ${file}`,
  );
  const pageUrl = new URL(
    base + file.slice(output.length + 1).replace(/index\.html$/, ""),
    origin,
  );
  const refs = [...content.matchAll(/(?:href|src|content)="([^"]+)"/g)].map(
    (match) => match[1],
  );
  for (const match of content.matchAll(/srcset="([^"]+)"/g))
    refs.push(...match[1].split(",").map((item) => item.trim().split(/\s/)[0]));
  for (const match of content.matchAll(/url\(([^)]+)\)/g))
    refs.push(match[1].replace(/^["']|["']$/g, ""));
  for (const ref of refs) {
    if (/^(data:|mailto:|tel:)/.test(ref)) continue;
    if (!(
      ref.startsWith("/") ||
      ref.startsWith("#") ||
      ref.startsWith(origin) ||
      (extname(file) === ".css" && !ref.includes(":"))
    ))
      continue;
    const url = new URL(ref.replaceAll("&amp;", "&"), pageUrl);
    if (url.origin !== origin) continue;
    const path = await target(url);
    if (url.hash) {
      const document = await readFile(path, "utf8");
      assert(
        document.includes(`id="${decodeURIComponent(url.hash.slice(1))}"`),
        `Missing fragment ${url}`,
      );
    }
    references++;
  }
  if (file.endsWith("index.html")) {
    assert(
      content.includes(`rel="canonical" href="${origin}${base}"`),
      "Home canonical differs from configured base",
    );
  }
}
for (const asset of JSON.parse(
  await readFile("src/data/assets.json", "utf8"),
)) {
  const bytes = await readFile(resolve("..", asset.source));
  assert.equal(
    createHash("sha256").update(bytes).digest("hex"),
    asset.sha256,
    `Original asset changed: ${asset.source}`,
  );
}
const sitemap = await readFile(join(output, "sitemap.xml"), "utf8");
assert(sitemap.includes(`<loc>${origin}${base}</loc>`));
console.log(
  `PASS: ${references} internal references; ${files.length} artifact files; ${base} canonicals/sitemap; consumed original hashes; no symlinks or local paths.`,
);
