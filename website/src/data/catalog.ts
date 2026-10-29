import evidence from "./evidence.json";
import features from "./features.json";
import compatibility from "./compatibility.json";
import roadmap from "./roadmap.json";
import release from "./release.json";
import media from "./media.json";
import history from "./release-history.json";
import { validateCatalog } from "../lib/schemas.mjs";
export const catalog = validateCatalog({
  evidence,
  features,
  compatibility,
  roadmap,
  release,
  media,
  history,
});
export const stable = catalog.release;
export const evidenceFor = (ids: string[]) =>
  ids.map((id) => catalog.evidence.find((e) => e.id === id)!);
