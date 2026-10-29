import { readFile, stat } from "node:fs/promises";
import { createHash } from "node:crypto";
import { resolve } from "node:path";
import { validateCatalog } from "../src/lib/schemas.mjs";
export async function loadCatalog() {
  const values = await Promise.all(
    [
      "evidence",
      "features",
      "compatibility",
      "roadmap",
      "release",
      "media",
      "release-history",
    ].map(async (name) =>
      JSON.parse(await readFile(`src/data/${name}.json`, "utf8")),
    ),
  );
  return Object.fromEntries(
    [
      "evidence",
      "features",
      "compatibility",
      "roadmap",
      "release",
      "media",
      "history",
    ].map((k, i) => [k, values[i]]),
  );
}
const c = validateCatalog(await loadCatalog());
for (const e of c.evidence) if (e.path) await stat(resolve("..", e.path));
for (const m of c.media) {
  const path = resolve("..", m.sourcePath);
  if (!path.startsWith(resolve("..") + "/"))
    throw new Error("Media escapes repository");
  const hash = createHash("sha256")
    .update(await readFile(path))
    .digest("hex");
  if (hash !== m.sourceHash) throw new Error(`Media source changed: ${m.id}`);
}
console.log(
  `Validated ${c.features.length} features, ${c.compatibility.length} environments, ${c.roadmap.length} roadmap items, stable release and media provenance.`,
);
