// Local QA server: directory routes, gzip, and a real static 404; never an SPA fallback.
import { createServer } from "node:http";
import { readFile, stat } from "node:fs/promises";
import { resolve, extname, sep } from "node:path";
import { gzipSync } from "node:zlib";
const root = resolve("dist");
const base = process.env.SITE_BASE || "/Radivine/";
const port = Number(process.env.SITE_TEST_PORT || 4321);
const types = {
  ".html": "text/html; charset=utf-8",
  ".xml": "application/xml",
  ".css": "text/css",
  ".js": "text/javascript",
  ".png": "image/png",
  ".webp": "image/webp",
  ".woff2": "font/woff2",
  ".txt": "text/plain; charset=utf-8",
};
createServer(async (req, res) => {
  try {
    const url = new URL(req.url, "http://localhost");
    let file = resolve(
      root,
      decodeURIComponent(url.pathname.slice(base.length)) || ".",
    );
    let status = 200;
    if (
      !url.pathname.startsWith(base) ||
      !(file === root || file.startsWith(root + sep))
    ) {
      status = 404;
    } else {
      try {
        if ((await stat(file)).isDirectory()) {
          if (!url.pathname.endsWith("/")) {
            res.writeHead(301, { Location: url.pathname + "/" + url.search });
            res.end();
            return;
          }
          file = resolve(file, "index.html");
        }
        await stat(file);
      } catch {
        status = 404;
      }
    }
    if (status === 404) file = resolve(root, "404.html");
    let bytes = await readFile(file);
    const headers = {
      "Content-Type": types[extname(file)] || "application/octet-stream",
      "Cache-Control": "no-cache",
      Vary: "Accept-Encoding",
    };
    if (
      /gzip/.test(req.headers["accept-encoding"] || "") &&
      /\.(html|xml|css|js|txt)$/.test(file)
    ) {
      bytes = gzipSync(bytes);
      headers["Content-Encoding"] = "gzip";
    }
    headers["Content-Length"] = bytes.length;
    res.writeHead(status, headers);
    res.end(req.method === "HEAD" ? undefined : bytes);
  } catch {
    res.writeHead(500);
    res.end("Static QA server error");
  }
}).listen(port, "127.0.0.1", () =>
  console.log(`Static QA: http://127.0.0.1:${port}${base}`),
);
