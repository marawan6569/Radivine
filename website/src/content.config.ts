import { defineCollection } from "astro:content";
import { file, glob } from "astro/loaders";
import {
  featureSchema,
  compatibilitySchema,
  roadmapSchema,
  evidenceSchema,
  mediaSchema,
  guideSchema,
} from "./lib/schemas.mjs";
export const collections = {
  features: defineCollection({
    loader: file("src/data/features.json"),
    schema: featureSchema,
  }),
  compatibility: defineCollection({
    loader: file("src/data/compatibility.json"),
    schema: compatibilitySchema,
  }),
  roadmap: defineCollection({
    loader: file("src/data/roadmap.json"),
    schema: roadmapSchema,
  }),
  evidence: defineCollection({
    loader: file("src/data/evidence.json"),
    schema: evidenceSchema,
  }),
  media: defineCollection({
    loader: file("src/data/media.json"),
    schema: mediaSchema,
  }),
  guides: defineCollection({
    loader: glob({ pattern: "**/*.md", base: "./src/content/guides" }),
    schema: guideSchema,
  }),
};
