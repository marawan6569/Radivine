# Radivine website

Local-only W2 product content and installation site, preserving the W1 foundation and W1.5 motion homepage. No deployment workflow or hosting registration is included. W0 decisions: [contract](../docs/website/W0-contract.md). W1.5 direction: [radial motion](../docs/website/W1.5-motion.md). W2 validation: [report](../docs/test-reports/W2.md). W3–W5 require separate authorization.

Use Node **24.22.1** (`nvm use` with `.nvmrc`), npm, and:

```sh
cd website
npm ci
npm run dev
# Open the printed URL with /Radivine/ (normally http://localhost:4321/Radivine/).
npm run check
npm run test:content
npm run format:check
npm run build
npm run check:links
npm run preview
```

The static artifact is `dist/`. Default origin `https://marawan6569.github.io` and base `/Radivine/` are candidate hosting configuration, not a live-site claim. To test root hosting:

```sh
SITE_BASE=/ npm run build
SITE_BASE=/ npm run check:links
```

Build again without overrides before reviewing the GitHub Pages candidate. `SITE_ORIGIN` overrides the origin for a future domain. Use `withBase` for all public paths and local anchors; imported Astro assets already have the base applied. Never prefix an imported image URL again.

Browser checks (local Chromium):

```sh
npx playwright install chromium
npm test
```

Playwright starts its own production preview server after a default-base build. Routine test artifacts are ignored. Curated W1.5 visual review media is versioned with the report. Browser checks require the browser's OS libraries; automated checks do not replace a manual screen-reader or Safari review.

## Architecture and boundaries

Astro static HTML, strict TypeScript, Tailwind 4 Vite integration; no framework, backend, visitor API calls, analytics or client router. `src/styles/tokens.css` owns semantic theme variables. `SiteLayout` owns metadata, no-flash theme initialization and the shell. Native disclosure navigation works without JavaScript. Theme preference is the only browser-local persisted state, under `radivine:theme`; blocked storage is tolerated. The website theme does not change application settings.

W1.5 home sections provide the motion-driven introduction. W2 adds `/download/`, `/compatibility/`, `/guide/`, `/guide/configuration/`, `/guide/troubleshooting/`, `/roadmap/`, `/contribute/` and `/licenses/`. Every route produces static HTML; native disclosures, tables, commands and links remain useful without JavaScript. Copy buttons are optional enhancements. The interactive demo is W3. Validation workflows and publication remain W4/W5; no CI files are added here.

`src/lib/schemas.mjs` provides strict Zod structures and semantic checks shared by Astro content collections and validation scripts. JSON catalogs in `src/data/` separate implementation, evidence, compatibility and roadmap status. Markdown guides in `src/content/guides/` have validated frontmatter and cross-references. Repository evidence is pinned to audited commits. New desktop verification needs a dated report and named environment; source inspection is not manual testing.

`src/data/release.json` is the sole current-stable manifest. `src/lib/install.ts`, download assets and SoftwareApplication metadata derive versions, URLs and commands from it. `npm run check:release` performs read-only GitHub verification of the pinned tag, commit, release date, asset metadata, checksum file and freshly downloaded package bytes. It reports the latest observed tag but never edits the manifest. This network check is deliberately separate from offline builds. A future release update needs an explicit audit of metadata, checksum, package requirements, claims, guides and history; do not auto-promote GitHub's latest release.

`npm run build` validates catalog cross-references and media source hashes before Astro rendering. `npm run test:content` exercises invalid support/release/evidence combinations. Guide examples are real TOML accepted by the repository parsers; rerun parser checks when changing them, without executing configured commands. The [W2 media handoff](../docs/website/W2-media.md) records historical media exclusions and required owner captures. Current public visuals remain honestly labeled illustrations.

## Assets

`src/data/assets.json` records consumed original hashes and licenses. The original logo is imported directly from `../assets/branding/logo-icon.png` and optimized by Astro into build output. Originals are never modified. `src/assets/inter-latin.woff2` is a website-only Latin subset of the repository's Inter variable font, generated with FontTools `pyftsubset --flavor=woff2` (Latin, punctuation, common symbols, full layout features). It retains weight variation; OFL license is in `public/notices/`. No external font requests. No old screenshot is represented as current. The hero and workflow use clearly captioned radial illustrations; feature studies are illustrations too, not native application UI.

Inputs outside website: `assets/branding/logo-icon.png`, `assets/fonts/Inter-Variable.ttf`, `assets/third_party/OFL-Inter.txt`, `LICENSE-MIT`, `LICENSE-APACHE`; product copy derives from `docs/website-plan.md` / W0. A future website workflow must track consumed asset inputs without modifying Rust CI or releases.

Before changing copied/derived assets, verify provenance and license and update the hash registry. The capture checklist in W0 is the gate for replacing the media placeholder. Do not regenerate any protected logo/icon or invent application UI.

Reference integration: [Astro styles](https://docs.astro.build/en/guides/styling/) and [Tailwind Astro](https://tailwindcss.com/docs/installation/framework-guides/astro). Node selected from the [official LTS release line](https://github.com/nodejs/Release).

## Project map

```text
website/
  astro.config.mjs, tsconfig.json, package.json, package-lock.json
  public/notices/                project licenses and Inter OFL
  src/assets/                    local Inter WOFF2 derivative
  src/components/layout/         brand, header, footer, theme select
  src/components/product/        hero, workflow story, illustrated features
  src/components/motion/          reusable radial SVG illustration
  src/components/ui/             buttons and section headings
  src/components/data/           evidence, support matrix, roadmap, commands, docs nav
  src/data/                      validated features, evidence, releases, roadmap, media
  src/content/                   Markdown guides and parser-checked TOML examples
  src/layouts/SiteLayout.astro    HTML, SEO, early theme setup, shared shell
  src/lib/urls.ts                base-aware internal paths
  src/pages/                     homepage, eight content routes, 404, sitemap
  src/scripts/theme.ts           preference, OS and cross-tab synchronization
  src/scripts/motion.ts           event-driven visual enhancement and cleanup
  src/styles/                    semantic tokens and shared styles
  scripts/check-links.mjs         artifact URL/fragment/asset/provenance checks
  tests/foundation.spec.ts        production-browser behavior and axe checks
  tests/motion.spec.ts            workflow, reduced motion, stability, theme contrast
  tests/content-pages.spec.ts     all new routes, axe, no-JS, clipboard, SEO, themes
  tests/content.test.mjs          schema and cross-reference rejection cases
  scripts/capture-review.mjs      reproducible screenshots, videos, local metrics
  scripts/capture-content-review.mjs  W2 full-page screenshots and layout metrics
```

Tests also run against root-hosted output with `SITE_BASE=/ npm test` after a root build. Keep build and tests sequential. Astro 7 can auto-background its server in agent environments; the Playwright command uses `--ignore-lock` so the runner owns and stops the foreground process. For an ordinary manually started background server, `npx astro preview stop` stops it.

## W1.5 visual review

The [review report](../docs/test-reports/W1.5.md) links desktop/mobile, Light/Dark screenshots and short WebM recordings. Full-page overviews use the reduced-motion layout; hero and branch stills and recordings use normal motion. No application screenshot is fabricated.

Run the production preview and capture in separate terminals:

```sh
npm run build
npm run preview -- --host 127.0.0.1 --port 4332 --ignore-lock
# In another terminal, from website/:
node scripts/capture-review.mjs
```

`REVIEW_URL` overrides the URL (including the base path); `REVIEW_OUTPUT` overrides the output directory, which defaults to `docs/test-reports/media/W1.5`. The script deliberately replaces its named review files. Desktop screenshots use 1440×960; videos scale to 960×640 for repository size. Mobile uses 390×844. Metrics come from local unthrottled Chromium with video capture, not field data or a Lighthouse score. The recorder's interpolated scrolling is test machinery, not application behavior.

Use `SITE_TEST_PORT=4331 npm test` to avoid an existing preview on the default port. Keep root/project builds and tests sequential. The original W1 shell tests remain in place; motion tests cover both scroll directions, live reduced-motion changes, touch/short viewport fallback, no JavaScript, settled animation state, CLS, and immediate theme contrast.

For W2 screenshots, serve the default-base production build on 4332, then run `node scripts/capture-content-review.mjs` from `website/`. It captures home, download, compatibility, configuration and roadmap at both desktop/mobile sizes and both system color schemes. `REVIEW_URL` and `REVIEW_OUTPUT` override defaults; output defaults to `docs/test-reports/media/W2`. No native desktop application is exercised by this browser capture.

## W3 interactive practice

The homepage `#try` section is a browser-only simulation. Its loader imports the controller near the viewport; no framework, native file access, conversion, clipboard write or visitor API request is involved. The hero and scroll story remain decorative W1.5 components.

- `src/demo/contexts.ts`: versioned sample menus and real availability reasons.
- `geometry.ts`: application radii, polar sectors, Branching candidate placement/corridors and Concentric annuli.
- `model.ts`: serializable reducer with injected time, input source, generation, exposure/intent guards and strict release checks.
- `controller.ts`: scoped keyboard/pointer adapters, pointer capture, one deadline timer and lifecycle cleanup.
- `view.ts`: keyed SVG layers, native-button action list, focus and announcements.
- `outcomes.ts`: side-effect-free explanations of desktop actions.

Read [the fidelity contract](../docs/website/W3-demo.md) and [verification/media report](../docs/test-reports/W3.md) before changing behavior. Source hashes in `tests/fixtures/demo-source.json` are a build gate. A product/source change requires review of fixtures and differences, not an automatic hash refresh.

```sh
npm run test:demo
npm run check:demo
# Optional: regenerate expectations from the unchanged Rust wheel crate.
# Cargo and cached dependencies must be available. Uses and deletes a temporary crate.
node scripts/capture-native-reference.mjs
npx prettier --write tests/fixtures/demo-native-traces.json
npm run test:demo
```

The native reference emitter never creates a window or executes an action. Review its JSON diff before committing. It verifies bounded engine paths, not live X11/desktop integration.

For screenshot/video review, build and serve the default base on 4332, then run `node scripts/capture-demo-review.mjs`. `REVIEW_URL` and `REVIEW_OUTPUT` override defaults. The script replaces its named W3 captures and emits measurements; recordings show browser practice, never native application UI. The root/project-base builds and tests must remain sequential because they share `dist/`.
