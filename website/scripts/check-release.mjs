import { readFile } from "node:fs/promises";
import { createHash } from "node:crypto";
import assert from "node:assert/strict";
import { releaseSchema } from "../src/lib/schemas.mjs";
const release = releaseSchema.parse(
  JSON.parse(await readFile("src/data/release.json", "utf8")),
);
async function request(url) {
  const response = await fetch(url, {
    headers: {
      Accept: "application/vnd.github+json",
      "User-Agent": "Radivine-website-read-only-verifier",
    },
    signal: AbortSignal.timeout(30000),
  });
  assert(response.ok, `${response.status} ${url}`);
  return response;
}
const api = "https://api.github.com/repos/marawan6569/Radivine";
const remote = await (
  await request(`${api}/releases/tags/${release.tag}`)
).json();
assert.equal(remote.draft, false);
assert.equal(remote.prerelease, false);
assert.equal(remote.tag_name, release.tag);
assert.equal(remote.published_at.slice(0, 10), release.publishedOn);
let object = (
  await (await request(`${api}/git/ref/tags/${release.tag}`)).json()
).object;
if (object.type === "tag")
  object = (await (await request(object.url)).json()).object;
assert.equal(object.type, "commit");
assert.equal(object.sha, release.sourceRevision);
const sums = await (await request(release.checksumsUrl)).text();
const results = [];
for (const asset of release.assets) {
  const published = remote.assets.find((a) => a.name === asset.name);
  assert(published, "Asset missing");
  assert.equal(published.browser_download_url, asset.url);
  assert.equal(published.size, asset.bytes);
  assert.equal(published.digest, `sha256:${asset.sha256}`);
  const line = sums
    .split(/\r?\n/)
    .find((s) => s.trim().split(/\s+/).at(-1) === asset.name);
  assert(line, "Checksum filename missing");
  assert.equal(line.trim().split(/\s+/)[0], asset.sha256);
  const bytes = Buffer.from(await (await request(asset.url)).arrayBuffer());
  assert.equal(bytes.length, asset.bytes);
  assert.equal(createHash("sha256").update(bytes).digest("hex"), asset.sha256);
  results.push({
    name: asset.name,
    bytes: bytes.length,
    sha256: asset.sha256,
    metadataChecksumAndDownloadedBytesAgree: true,
  });
}
const latest = await (await request(`${api}/releases/latest`)).json();
console.log(
  JSON.stringify(
    {
      verifiedOn: new Date().toISOString(),
      pinned: release.tag,
      latestObserved: latest.tag_name,
      sourceRevision: object.sha,
      assets: results,
      note: "Read-only verification; no manifest update, installation, publication or deployment.",
    },
    null,
    2,
  ),
);
