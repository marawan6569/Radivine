import { readFile } from "node:fs/promises";
import { createHash } from "node:crypto";
import assert from "node:assert/strict";
const root = new URL("../../", import.meta.url);
const fixture = JSON.parse(
  await readFile(
    new URL("../tests/fixtures/demo-source.json", import.meta.url),
  ),
);
const release = JSON.parse(
  await readFile(new URL("../src/data/release.json", import.meta.url)),
);
assert.equal(
  fixture.productVersion,
  release.version,
  "Review demo fixtures for the new product version",
);
assert.equal(
  fixture.sourceRevision,
  release.sourceRevision,
  "Review demo fixtures for the new release source",
);
for (const [path, expected] of Object.entries(fixture.sourceHashes)) {
  const hash = createHash("sha256")
    .update(await readFile(new URL(path, root)))
    .digest("hex");
  assert.equal(
    hash,
    expected,
    `Demo source drift: ${path}; review behavior and tests before updating the fixture`,
  );
}
console.log(
  `Demo fixtures match ${fixture.sourceRevision} / v${fixture.productVersion}: ${Object.keys(fixture.sourceHashes).length} source files checked`,
);
