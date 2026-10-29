# W5 — Project Pages deployment and rollback

The owner explicitly authorized the initial launch under CONDITIONAL GO on 2026-10-28. The only production destination is **https://marawan6569.github.io/Radivine/**. The W4 runbook is historical. No custom domain is authorized for W5.

## Configuration and release gate

Astro defaults to origin `https://marawan6569.github.io`, base `/Radivine/`, static output and trailing-slash directory routes. The normalized Astro base supports `/Radivine`; the public URL ends with `/`. Keep the existing origin/base overrides for a future separately authorized migration. The root build is a validation configuration only and is never uploaded to Pages.

The owner reports physical Android smartphone and 13-inch tablet testing and a positive external UI/UX designer review. Detailed device/browser versions and case-level records were not supplied. Representative native-demo comparison, screen-reader/actual zoom acceptance and remaining Firefox/WebKit/Safari coverage are accepted unresolved risks, not passing tests. See [W5 evidence](../test-reports/W5.md) and [W4.5 instructions](../../website/reviews/w4/manual-acceptance.md). Do not ask the owner to repeat these checks for W5.

## Publication procedure

1. Validate and review the website branch. Use the normal pull request into `main`, preserving history and all existing required checks/reviews. Do not use an administrative bypass.
2. Configure repository Pages source to **GitHub Actions** (`build_type: workflow`), leave the custom domain empty and enforce HTTPS. Restrict `github-pages` deployment branches to `main`. Preserve any existing reviewer requirements. If permissions prevent configuration, stop and ask the owner to perform that exact settings operation.
3. `.github/workflows/website.yml` validates project and root builds. Builds have read-only repository permissions, no persisted checkout credentials and normal `pull_request` events. Both validation jobs must succeed. Only `website/dist` from the project build can become the `radivine-website-pages` artifact.
4. Review the merged main SHA. The W5 owner authorization permits this initial deployment once critical checks pass. Set repository Actions variable `WEBSITE_APPROVED_SHA` to the full approved main SHA; dispatch **Website validation and Pages** on `main`. A newer main commit cannot publish with a stale approval. PRs, forks and feature-branch dispatches cannot publish.
5. The deployment job alone receives `pages: write` / `id-token: write`, uses the `github-pages` environment and verifies workflow hosting, no CNAME, HTTPS and the exact project URL before deployment. It cannot auto-enable Pages. Action references are pinned. Rust CI, release workflows and existing release assets remain untouched.
6. Require a successful Pages deployment and anonymous live smoke checks before claiming publication. From `website/`, restore the production build and run `node scripts/smoke-production.mjs`, followed by `npm run check:release`. The smoke script compares all served files to the local artifact, verifies every sitemap route, canonical/OG metadata, CSS/JS/images/fonts, static 404, themes, both demo layouts and no-JS download content. Record the exact deployed SHA, run URL, Pages metadata, artifact identity and results. HTTP-to-HTTPS redirection is checked separately.

GitHub's [custom Pages workflow requirements](https://docs.github.com/en/pages/getting-started-with-github-pages/using-custom-workflows-with-github-pages) and [Pages configuration API](https://docs.github.com/en/rest/pages/pages) were checked during W5.

## Rollback and hold

Create a normal reviewed revert/restoration PR on `main` for the website source, lockfile and content. Preserve the guarded workflow and application/release files. Run validation, merge only after required checks/reviews, approve the resulting new full main SHA in `WEBSITE_APPROVED_SHA`, then dispatch the website workflow on main and repeat live smoke checks. Do not rewrite history, force push, deploy a feature branch or modify application releases.

This is the first production deployment: there is no earlier known-good public website to restore. After launch, use its recorded source as the baseline for subsequent rollback. Clearing `WEBSITE_APPROVED_SHA` prevents new deployments but does **not** remove the already published site. Unpublishing requires an explicit owner instruction; never substitute an untested build or DNS change.
