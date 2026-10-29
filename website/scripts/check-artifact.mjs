import { readdir, lstat, readFile } from "node:fs/promises";
import { extname, join } from "node:path";
import { gzipSync } from "node:zlib";
import assert from "node:assert/strict";
const allowed = new Set([
  ".html",
  ".xml",
  ".css",
  ".js",
  ".png",
  ".webp",
  ".avif",
  ".woff2",
  ".txt",
]);
const inventory = [];
async function walk(dir) {
  for (const name of await readdir(dir)) {
    const path = join(dir, name),
      st = await lstat(path);
    assert(
      !st.isSymbolicLink() && (st.isDirectory() || st.nlink === 1),
      `Link in artifact: ${path}`,
    );
    assert(
      !/^(node_modules|target|\.git|\.env|src)$/.test(name),
      `Forbidden artifact: ${path}`,
    );
    if (st.isDirectory()) {
      await walk(path);
      continue;
    }
    assert(allowed.has(extname(name)), `Unexpected artifact type: ${path}`);
    const bytes = await readFile(path);
    if (/\.(html|xml|css|js|txt)$/.test(name)) {
      assert(
        !/file:\/\/|\/home\/mfazora|sourceMappingURL|-----BEGIN (?:RSA |OPENSSH |EC )?PRIVATE KEY-----|gh[pousr]_[A-Za-z0-9]{36}|AKIA[0-9A-Z]{16}/.test(
          bytes.toString(),
        ),
        `Local path, source map or credential pattern: ${path}`,
      );
    }
    inventory.push({
      file: path.slice(5),
      bytes: bytes.length,
      gzip: gzipSync(bytes).length,
    });
  }
}
await walk("dist");
const html = await readFile("dist/index.html", "utf8");
const inline = [...html.matchAll(/<script\b([^>]*)>([\s\S]*?)<\/script>/g)]
  .filter((m) => !m[1].includes("src=") && !m[1].includes("ld+json"))
  .map((m) => m[2])
  .join("\n");
const initialFiles = [...html.matchAll(/<script\b[^>]*src="([^"]+)"/g)].map(
  (m) => m[1].split("/").at(-1),
);
const initialGzip =
  gzipSync(inline).length +
  inventory
    .filter((f) => initialFiles.includes(f.file.split("/").at(-1)))
    .reduce((sum, f) => sum + f.gzip, 0);
const demoGzip = inventory
  .filter((f) => /controller\..*\.js$/.test(f.file))
  .reduce((sum, f) => sum + f.gzip, 0);
assert(initialGzip <= 20 * 1024, `Initial JS exceeds 20 KiB: ${initialGzip}`);
assert(
  demoGzip > 0 && demoGzip <= 60 * 1024,
  `Demo exceeds 60 KiB or missing: ${demoGzip}`,
);
console.log(
  JSON.stringify(
    {
      initialGzip,
      demoGzip,
      totalRaw: inventory.reduce((s, f) => s + f.bytes, 0),
      inventory,
      note: "Gzip sizes are estimates; browser transfer measured separately. Pattern scan is not a complete secret detector.",
    },
    null,
    2,
  ),
);
