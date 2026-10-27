import { z } from "astro/zod";
const text = z.string().min(1);
const id = z.string().regex(/^[a-z0-9][a-z0-9-]*$/);
const date = z
  .string()
  .regex(/^\d{4}-\d{2}-\d{2}$/)
  .refine(
    (s) =>
      Number.isFinite(Date.parse(s)) &&
      new Date(s).toISOString().slice(0, 10) === s,
    "Invalid calendar date",
  );
const url = z.url().refine((s) => s.startsWith("https://"), "HTTPS required");
const version = z.string().regex(/^\d+\.\d+\.\d+$/);
const refs = z.array(text).min(1);
export const verificationSchema = z
  .object({
    method: z.enum([
      "source-inspection",
      "unit-property",
      "fake-platform",
      "headless-integration",
      "xvfb",
      "recorded-manual",
      "live-release-metadata",
    ]),
    evidenceIds: refs,
    date,
    environment: text.optional(),
  })
  .strict()
  .refine(
    (v) => v.method !== "recorded-manual" || !!v.environment,
    "Manual results require an environment",
  );
export const evidenceSchema = z
  .object({
    id,
    kind: z.enum([
      "source",
      "test",
      "manual-report",
      "documentation",
      "release",
    ]),
    path: text.optional(),
    revision: z
      .string()
      .regex(/^[a-f0-9]{40}$/)
      .optional(),
    url,
    observedOn: date,
    productVersion: version,
    scope: text,
  })
  .strict();
export const featureSchema = z
  .object({
    id,
    title: text,
    summary: text,
    implementation: z.enum(["implemented", "partial", "absent"]),
    since: version.optional(),
    constraints: z.array(text).min(1),
    verification: z.array(verificationSchema).min(1),
    group: z.enum(["wheel", "tools", "personalize"]),
  })
  .strict()
  .refine(
    (f) => (f.implementation === "absent" ? !f.since : !!f.since),
    "Shipping features need a release; absent features cannot claim one",
  );
const capability = z
  .object({
    availability: z.enum([
      "available",
      "conditional",
      "unavailable",
      "unknown",
    ]),
    qualification: text,
  })
  .strict();
export const compatibilitySchema = z
  .object({
    id,
    os: text,
    osVersion: text,
    architecture: text,
    desktop: text,
    fileManager: text,
    session: z.enum(["x11", "wayland-xwayland", "wayland-native", "other"]),
    status: z.enum(["verified", "conditional", "unverified", "unsupported"]),
    publicLabel: text,
    packageStatus: z.enum([
      "published",
      "install-tested",
      "source-only",
      "none",
      "unknown",
    ]),
    capabilities: z
      .object({
        drag: capability,
        invoke: capability,
        concentric: capability,
        properties: capability,
      })
      .strict(),
    note: text,
    verification: z.array(verificationSchema).min(1),
    lastReviewed: date,
    manualChecks: z.array(text).min(1),
  })
  .strict()
  .superRefine((r, c) => {
    if (r.session === "wayland-native" && r.status !== "unsupported")
      c.addIssue({
        code: "custom",
        message: "Native Wayland wheel remains unsupported",
      });
    if (
      r.session.startsWith("wayland") &&
      r.capabilities.drag.availability !== "unavailable"
    )
      c.addIssue({
        code: "custom",
        message: "Wayland drag cannot be supported",
      });
    if (
      r.session === "wayland-native" &&
      r.capabilities.invoke.availability !== "unavailable"
    )
      c.addIssue({
        code: "custom",
        message: "No native Wayland wheel backend",
      });
    if (
      r.status === "verified" &&
      !r.verification.some((v) => v.method === "recorded-manual")
    )
      c.addIssue({
        code: "custom",
        message: "Verified desktop rows require recorded manual evidence",
      });
  });
export const roadmapSchema = z
  .object({
    id,
    title: text,
    summary: text,
    status: z.enum(["shipped", "exploring", "deferred"]),
    evidenceIds: refs,
    lastReviewed: date,
    shippedVersion: version.optional(),
    releasedOn: date.optional(),
    releaseUrl: url.optional(),
  })
  .strict()
  .refine(
    (r) =>
      r.status === "shipped"
        ? !!(r.shippedVersion && r.releasedOn && r.releaseUrl)
        : !(r.shippedVersion || r.releasedOn || r.releaseUrl),
    "Only shipped items have released versions/dates; future commitments are not authorized",
  );
export const releaseSchema = z
  .object({
    version,
    tag: text,
    publishedOn: date,
    verifiedOn: date,
    sourceRevision: z.string().regex(/^[a-f0-9]{40}$/),
    channel: z.literal("stable"),
    draft: z.literal(false),
    prerelease: z.literal(false),
    releaseUrl: url,
    checksumsUrl: url,
    notesUrl: url,
    assets: z
      .array(
        z
          .object({
            name: text,
            url,
            bytes: z.number().int().positive(),
            sha256: z.string().regex(/^[a-f0-9]{64}$/),
            os: z.literal("linux"),
            architecture: z.literal("amd64"),
            package: z.literal("deb"),
          })
          .strict(),
      )
      .length(1),
    verification: z.array(verificationSchema).min(1),
  })
  .strict()
  .superRefine((r, c) => {
    const base = `https://github.com/marawan6569/Radivine/releases`;
    const asset = r.assets[0];
    if (
      r.tag !== `v${r.version}` ||
      r.releaseUrl !== `${base}/tag/${r.tag}` ||
      r.checksumsUrl !== `${base}/download/${r.tag}/SHA256SUMS.txt` ||
      r.notesUrl !== `${base}/download/${r.tag}/RELEASE_NOTES_${r.tag}.md` ||
      asset.name !== `radivine_${r.version}_amd64.deb` ||
      asset.url !== `${base}/download/${r.tag}/${asset.name}`
    )
      c.addIssue({
        code: "custom",
        message: "Release identity and asset URLs must agree",
      });
  });
export const mediaSchema = z
  .object({
    id,
    sourcePath: text,
    sourceHash: z.string().regex(/^[a-f0-9]{64}$/),
    width: z.number().int().positive(),
    height: z.number().int().positive(),
    alt: text,
    caption: text,
    kind: z.enum([
      "live-desktop",
      "headless-export",
      "sandbox",
      "brand",
      "illustration",
    ]),
    productVersion: version.nullable(),
    environment: text,
    license: text,
    evidenceIds: refs,
  })
  .strict()
  .refine(
    (m) => m.kind !== "live-desktop" || !!m.productVersion,
    "Desktop captures require a known version",
  );
export const guideSchema = z
  .object({
    title: text,
    description: text,
    slug: text,
    lang: z.literal("en"),
    updatedOn: date,
    productVersion: version,
    evidenceIds: refs,
    draft: z.literal(false),
  })
  .strict();

export function validateGuide(input, catalog) {
  const guide = guideSchema.parse(input);
  if (
    guide.productVersion !== catalog.release.version ||
    guide.evidenceIds.some((id) => !catalog.evidence.some((e) => e.id === id))
  )
    throw new Error("Invalid guide references");
  return guide;
}

/** Shared semantic checks run before builds and from every catalog consumer. */
export function validateCatalog(input) {
  const c = {
    evidence: z.array(evidenceSchema).parse(input.evidence),
    features: z.array(featureSchema).parse(input.features),
    compatibility: z.array(compatibilitySchema).parse(input.compatibility),
    roadmap: z.array(roadmapSchema).parse(input.roadmap),
    release: releaseSchema.parse(input.release),
    media: z.array(mediaSchema).parse(input.media),
    history: z
      .array(z.object({ version, date, url }).strict())
      .parse(input.history),
  };
  for (const [name, records] of Object.entries(c))
    if (Array.isArray(records)) {
      const ids = records.map((r) => r.id ?? r.version);
      if (new Set(ids).size !== ids.length)
        throw new Error(`Duplicate IDs in ${name}`);
    }
  const evidence = new Map(c.evidence.map((e) => [e.id, e]));
  const checkRefs = (ids) => {
    for (const id of ids)
      if (!evidence.has(id)) throw new Error(`Unknown evidence: ${id}`);
  };
  for (const record of [
    ...c.features,
    ...c.compatibility,
    c.release,
    ...c.roadmap,
    ...c.media,
  ]) {
    if (record.evidenceIds) checkRefs(record.evidenceIds);
    for (const v of record.verification ?? []) {
      checkRefs(v.evidenceIds);
      if (
        v.method === "recorded-manual" &&
        !v.evidenceIds.some((id) => evidence.get(id).kind === "manual-report")
      )
        throw new Error("Manual verification needs a report");
    }
  }
  for (const e of c.evidence)
    if (
      e.path &&
      (!e.revision || !e.url.includes(`/blob/${e.revision}/${e.path}`))
    )
      throw new Error(`Unpinned evidence: ${e.id}`);
  for (const r of c.roadmap.filter((r) => r.status === "shipped"))
    if (
      !c.history.some(
        (h) =>
          h.version === r.shippedVersion &&
          h.date === r.releasedOn &&
          h.url === r.releaseUrl,
      )
    )
      throw new Error(`Unknown published release: ${r.id}`);
  for (const f of c.features)
    if (f.since && !c.history.some((h) => h.version === f.since))
      throw new Error(`Unknown feature release: ${f.id}`);
  for (const m of c.media)
    if (
      m.productVersion &&
      !c.history.some((h) => h.version === m.productVersion)
    )
      throw new Error(`Unknown media release: ${m.id}`);
  if (
    !c.history.some(
      (h) =>
        h.version === c.release.version &&
        h.date === c.release.publishedOn &&
        h.url === c.release.releaseUrl,
    )
  )
    throw new Error("Stable release missing from history");
  return c;
}
