import { test } from "node:test";
import assert from "node:assert/strict";
import { loadCatalog } from "../scripts/check-content.mjs";
import { validateCatalog } from "../src/lib/schemas.mjs";
const valid = await loadCatalog();
test("audited catalog accepts valid records", () =>
  assert.doesNotThrow(() => validateCatalog(valid)));
for (const [name, mutate] of [
  [
    "unknown evidence",
    (c) => c.features[0].verification[0].evidenceIds.push("missing"),
  ],
  ["duplicate IDs", (c) => c.features.push(c.features[0])],
  ["invalid calendar date", (c) => (c.release.publishedOn = "2026-02-30")],
  ["bad checksum", (c) => (c.release.assets[0].sha256 = "123")],
  [
    "wrong asset filename",
    (c) => (c.release.assets[0].name = "radivine_latest.deb"),
  ],
  [
    "wrong release URL",
    (c) => (c.release.releaseUrl = "https://example.com/release"),
  ],
  [
    "unsupported architecture",
    (c) => (c.release.assets[0].architecture = "arm64"),
  ],
  ["prerelease as stable", (c) => (c.release.prerelease = true)],
  ["draft as stable", (c) => (c.release.draft = true)],
  [
    "native Wayland claim",
    (c) => (c.compatibility[6].capabilities.invoke.availability = "available"),
  ],
  [
    "Wayland drag claim",
    (c) => (c.compatibility[5].capabilities.drag.availability = "available"),
  ],
  [
    "manual without environment",
    (c) => delete c.compatibility[0].verification[0].environment,
  ],
  [
    "manual without report",
    (c) => (c.compatibility[0].verification[0].evidenceIds = ["platform"]),
  ],
  ["verified without manual", (c) => (c.compatibility[1].status = "verified")],
  [
    "unpublished shipped version",
    (c) => (c.roadmap[0].shippedVersion = "9.0.0"),
  ],
  ["exploratory commitment", (c) => (c.roadmap[4].targetVersion = "0.4.0")],
  ["unversioned native media", (c) => (c.media[0].kind = "live-desktop")],
  [
    "unpinned source",
    (c) =>
      (c.evidence[0].url =
        "https://github.com/marawan6569/Radivine/blob/main/README.md"),
  ],
])
  test(`rejects ${name}`, () => {
    const c = structuredClone(valid);
    mutate(c);
    assert.throws(() => validateCatalog(c));
  });
