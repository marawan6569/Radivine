# Radivine website

Local-only W1 static foundation. No website deployment workflow or hosting registration is included. W0 decisions and capture requirements: [contract](../docs/website/W0-contract.md). W2–W5 require separate authorization.

Use Node **24.22.1** (`nvm use` with `.nvmrc`), npm, and:

```sh
cd website
npm ci
npm run dev
# Open the printed URL with /Radivine/ (normally http://localhost:4321/Radivine/).
npm run check
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

Playwright starts its own production preview server after a default-base build. Test artifacts are ignored. Browser checks require the browser's OS libraries; automated checks do not replace a manual screen-reader or Safari review.

## Architecture and boundaries

Astro static HTML, strict TypeScript, Tailwind 4 Vite integration; no framework, backend, visitor API calls, analytics or client router. `src/styles/tokens.css` owns semantic theme variables. `SiteLayout` owns metadata, no-flash theme initialization and the shell. Native disclosure navigation works without JavaScript. Theme preference is the only browser-local persisted state, under `radivine:theme`; blocked storage is tolerated. The website theme does not change application settings.

W1 home sections provide visual structure and brief repository-backed introduction only. Dedicated product/download/guide/compatibility/roadmap pages, content schemas, full release manifest, real media gallery and installation instructions are W2. The interactive demo is W3. Validation workflows and publication remain W4/W5; no CI files are added here.

## Assets

`src/data/assets.json` records consumed original hashes and licenses. The original logo is imported directly from `../assets/branding/logo-icon.png` and optimized by Astro into build output. Originals are never modified. `src/assets/inter-latin.woff2` is a website-only Latin subset of the repository's Inter variable font, generated with FontTools `pyftsubset --flavor=woff2` (Latin, punctuation, common symbols, full layout features). It retains weight variation; OFL license is in `public/notices/`. No external font requests. No old screenshot is represented as current; the hero explicitly reserves fresh desktop media with brand artwork.

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
  src/components/product/        hero and honest capture placeholder
  src/components/ui/             buttons and section headings
  src/data/                      source-asset hashes and site snapshot
  src/layouts/SiteLayout.astro    HTML, SEO, early theme setup, shared shell
  src/lib/urls.ts                base-aware internal paths
  src/pages/                     homepage, 404, sitemap
  src/scripts/theme.ts           preference, OS and cross-tab synchronization
  src/styles/                    semantic tokens and shared styles
  scripts/check-links.mjs         artifact URL/fragment/asset/provenance checks
  tests/foundation.spec.ts        production-browser behavior and axe checks
```

Tests also run against root-hosted output with `SITE_BASE=/ npm test` after a root build. Keep build and tests sequential. Astro 7 can auto-background its server in agent environments; the Playwright command uses `--ignore-lock` so the runner owns and stops the foreground process. For an ordinary manually started background server, `npx astro preview stop` stops it.
