# Website deployment and rollback — W4 review only

Publication remains disabled. W4 did not push, merge, change repository settings, enable Pages or invoke a deployment. The intended project URL is `https://marawan6569.github.io/Radivine/`; it is not a verified live website.

## Workflow boundaries

[website.yml](../../.github/workflows/website.yml) is separate from Rust CI and application release/package tooling. Both root and project configurations run format, Astro/TypeScript, content/demo units, source-drift checks, production builds, link/artifact checks, and Chromium/Firefox/WebKit Playwright checks. CI browser dependencies are installed only on the disposable Ubuntu runner. Local W4 could not execute Firefox/WebKit; passing the configured CI jobs still needs verification after an authorized push.

Top-level permissions are `contents: read`; checkout disables persisted credentials. PRs use `pull_request`, never `pull_request_target`; no secrets, publication tokens, Pages settings or package mutations enter the build. Test reports are ordinary artifacts. Only the project build may upload `website/dist` as `radivine-website-pages`, and only when the publication guards permit it. The artifact checker rejects links, unexpected extensions, known source/build directories, source maps, local paths and common credential patterns. This scan is not proof against every possible secret.

Only the dependent deployment job has `pages: write` and `id-token: write`. It runs in `github-pages`; `configure-pages` has automatic enablement disabled. PR checks cancel superseded checks in website-specific groups; deployment has a separate serialized group and does not cancel an active deployment or application release.

**Two literal `false &&` holds** prevent Pages artifact upload and deployment even if a repository variable is accidentally set. Future publication additionally requires the canonical repository, `refs/heads/main`, a push or manual dispatch event, and `WEBSITE_APPROVED_SHA` equal to the exact workflow commit. Fork PRs, same-repository PRs, feature-branch dispatches and unapproved newer main commits cannot pass those guards. There is no arbitrary-ref dispatch input, release tag trigger, workflow-run trigger or scheduled publication.

Action pins were resolved read-only from official action repositories on 2026-10-28. Local actionlint validates workflow syntax/expressions. Their operational behavior on GitHub, organization policies and environment protection settings remain W5 checks. A review of upstream documentation confirms artifact/dependency/environment requirements: [GitHub custom Pages workflows](https://docs.github.com/en/pages/getting-started-with-github-pages/using-custom-workflows-with-github-pages).

## Publication procedure — requires separate W5 authorization

1. Resolve the blocking/manual gates in [W4's report](../test-reports/W4.md). Review the exact website changes, report, intended URL and workflow. Do not infer approval from this document.
2. After explicit authorization, use the normal reviewed PR/merge process. Remove both literal holds in a separately reviewed change. Keep canonical-repository, main-branch, event and exact-SHA guards. Confirm that existing Rust checks and required branch protections remain intact.
3. An authorized maintainer configures Pages source **GitHub Actions**, confirms the intended domain/base, and restricts `github-pages` to main with the required human reviewer where supported. Never enable deployment for PR branches. Record any environment-policy capability limitations.
4. Review the exact merged main SHA and set `WEBSITE_APPROVED_SHA` to that full SHA only after its publication approval. Dispatch **Website release candidate** on main. A newer main commit requires a new approved SHA. Both build configurations and all available browser projects must succeed before the deploy job can run.
5. Record run URL, reviewed source SHA, Pages artifact digest, deployed URL and deployment ID. Confirm the Pages artifact contains only static website files; no review media, package, source tree, `node_modules`, `target`, caches or links.
6. Perform anonymous HTTPS smoke checks: home, download, compatibility, every nested guide, roadmap, contribution, licenses, unknown nested route (HTTP 404), images/fonts/JS, themes, keyboard/touch practice, real package/checksum URLs, canonical and sitemap. Compare package bytes with the manifest again. No install is necessary for website smoke testing. Record results and stop at the separately approved W5 boundary.

## Rollback

Prefer a normal reviewed revert commit on **main** restoring the last approved website source (including its lockfile and static data). Preserve the guarded current workflow, main-branch strategy and all Rust/application files. Rerun both builds and browser checks, obtain approval for that exact new main SHA, update `WEBSITE_APPROVED_SHA`, and dispatch on main. Do not use force-push, reset history, deploy a feature branch, or roll back application releases. An older workflow run may have different guards or dependencies, so it is not the default rollback path.

If the site must be held while a correction is prepared, an authorized maintainer clears the approved-SHA variable or restores the two literal holds through a normal commit. This prevents new deployments; it does not remove already public content. Any hosting takedown requires its own explicit instruction.

## Reproduce local validation

Use Node from `website/.nvmrc` and run commands from `website/`:

```sh
npm ci
npm run format:check
npm run check
npm run test:content
npm run test:demo
npm run build
npm run check:links
npm run check:artifact
npm test -- --workers=2
# On a host with all required browser dependencies:
SITE_BROWSERS=chromium,firefox,webkit npm test -- --workers=2
# Sequentially, because both configurations share dist:
SITE_BASE=/ npm run build
SITE_BASE=/ npm run check:links
SITE_BASE=/ npm run check:artifact
SITE_BASE=/ npm test -- --workers=2
# Restore the intended project artifact:
npm run build
```

`npm test` serves the production artifact with a loopback-only static server, gzip, directory routes and an actual HTTP 404 fallback. It does not use a development server or SPA rewrite. This models static hosting, not GitHub's CDN or cache headers. `SITE_TEST_PORT` avoids existing servers. It never reuses an unknown process.

For Lighthouse and controlled Chromium measurements, start `SITE_TEST_PORT=4345 npm run serve:static`, then in another terminal use `REVIEW_URL=http://127.0.0.1:4345/Radivine/ npm run audit:performance`. Run performance checks without simultaneous build/browser workloads. Defaults use three cold mobile Lighthouse runs and separate mobile/desktop measurements at 4× CPU, 150 ms latency and 1.6 Mbit/s. Raw reports stay in ignored `test-results/`; curated measurements belong in W4 evidence. They are lab data, not field INP or accessibility certification.
