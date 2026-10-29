import { stable } from "./catalog";
import { withBase } from "../lib/urls";
export const site = {
  name: "Radivine",
  version: stable.version,
  description:
    "A context-aware radial action wheel for Linux files. Preview text, convert images, and run your own configured commands. Built for X11.",
  github: "https://github.com/marawan6569/Radivine",
  release: stable.releaseUrl,
  source: `https://github.com/marawan6569/Radivine/blob/${stable.sourceRevision}`,
  guide: withBase("guide/"),
  contribute: `https://github.com/marawan6569/Radivine/blob/${stable.sourceRevision}/CONTRIBUTING.md`,
} as const;
