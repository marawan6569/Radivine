# W2 media audit and owner capture handoff

Reviewed 2026-10-27. No native application image was added to the public website. W1.5's approved hero, workflow and feature illustrations remain explicitly illustrative. The guide reuses the radial SVG through `ScreenshotFigure`, with a caption, accessible description and expandable provenance. `website/src/data/media.json` pins its source hash, dimensions, kind, environment and license; build validation detects changes.

The media manifest also inventories the three existing feature studies (text, image conversion and configuration), including their source hashes and SVG viewBox dimensions. They remain decorative with adjacent explanatory copy. The hero and workflow share the `RadialArt.astro` source recorded as `radial-study`; its variants are conceptual, not versioned UI. The unchanged brand image and font provenance remain in `assets.json`.

## Existing repository media

| Source | Known provenance | W2 decision |
| --- | --- | --- |
| `docs/screenshots/control-dashboard-dark.png` and `control-*.png` | Historical Control Center references; the dashboard visibly identifies v0.1.1. Exact capture environment is not established for every file. | Do not label as v0.3.0 or display as current UI. |
| `docs/screenshots/wheel-*.png` | Historical reference scenes; development paths and additional UI appear. Exact release/capture environment is not established for every image. | Not selected. |
| `docs/screenshots/viewer-*.png` | Existing viewer references, current-release provenance unconfirmed. | Not selected. |
| `docs/test-reports/screenshots/01-root-wheel-dark.png` through the original wheel review pack | Historical renderer/sandbox evidence, not a new stable desktop capture. | Retain in the original reports only. |
| `docs/test-reports/screenshots/n3-concentric/*.png` | N3 renderer/sandbox fixtures; debug geometry and sample data. Representative `02-concentric-2-levels-submenu-open.png` was visually inspected. | Do not represent as live v0.3.0 desktop UI. |
| `docs/test-reports/media/W1.5/*.webm` and PNGs | Website browser captures, not native application recordings. | Keep linked from W1.5's review; not a product gallery. |
| `docs/test-reports/media/W2/*.png` | Local Chromium captures of this website, 1440×960 and 390×844; system Light/Dark, reduced motion. | Website visual-review artifacts only. |

The historical Control Center dashboard and N3 submenu fixture were opened and inspected, in addition to reviewing W0's inventory and existing reports. No image was retouched, relabeled, regenerated or copied into a current-product gallery. This audit does not establish a version or environment for every historical image.

## Required current desktop captures

Capture the actual stable v0.3.0 application on the owner's Linux Mint 22.3 / Cinnamon X11 / Nemo / amd64 desktop. Use disposable, neutral fixtures and prepare the desktop to exclude personal paths, unrelated applications and private filenames.

1. Branching wheel: root → File Preview → Head/Tail, followed by the real read-only viewer. Include an enabled leaf, and show only actions appropriate to the selected file.
2. Concentric Rings: image selection → Convert → Image → a supported target. Show the chosen layout, sufficient logical workarea and actual submenu geometry.
3. Advanced Convert: actual quality/resize/output settings for a supported static image.
4. Jobs: a real queued/running/completed job and its per-item results using disposable inputs.
5. Control Center: Dashboard and Wheel Layout selection, including current version identification.
6. Light and Dark examples of the wheel and Control Center; short real desktop Drag + Shift and Invoke recordings if available.

For every selected item record original source path, SHA-256, pixel dimensions, capture date, exact version/revision, OS/desktop/file manager/session, compositor, monitor layout/scale, selected application theme/layout, fixture names, capture method, media kind, accurate alt text/caption, license and any crop/derivative history. Do not change version labels in pixels. A renderer export must be labeled as such and cannot certify desktop integration.

Review the optimized website crop, privacy, readable text, and matching menu/action behavior before switching the provenance record to a live desktop image. Preserve the original protected branding/icons. Missing captures do not block W2's content completion; they remain a manual media handoff, not a claim of successful desktop validation.
