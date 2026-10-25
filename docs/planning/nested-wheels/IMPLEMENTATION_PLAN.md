# Implementation plan — N1 through N6

Status: **Proposed; none of these milestones is authorized by the planning task.**
Baseline: v0.2.0 / `e6a342e`. Each milestone requires explicit owner authorization.
Complete its verification, report and commits, then stop. Authorization for N1
does not authorize N2. The implementation agent must not redesign the geometry
or relax safety/performance gates to finish a milestone.

Read [PRODUCT_SPEC.md](PRODUCT_SPEC.md), [INTERACTION_DESIGN.md](INTERACTION_DESIGN.md),
[ARCHITECTURE.md](ARCHITECTURE.md), [TEST_MATRIX.md](TEST_MATRIX.md), and
[ADR-NW-001.md](ADR-NW-001.md) before changes. Interaction rules are authoritative
for geometry/input; architecture owns APIs/integration; test matrix owns gates.
Any contradiction must be resolved in the plan before dependent implementation.

## Common checkpoint procedure

1. Inspect `git status` and recent history. Preserve completed work in focused
   Conventional Commits; do not stage unrelated work or duplicate existing commits.
   Respect the existing branch strategy; do not amend history or push.
2. Make the smallest complete change, review its diff, run formatting, relevant
   Clippy and focused tests, then commit. Repeat within the milestone. Every
   numbered unit below is a suggested commit boundary, not one giant final commit.
3. At the milestone gate run `cargo fmt --all --check`,
   `cargo clippy --workspace --all-targets -- -D warnings`,
   `cargo test --workspace -- --test-threads=1`, and dependency graph checks in an
   isolated test environment. Record skipped display/service branches separately.
   Tests requiring live desktop operations run only under the authorized desktop
   verification scope; never run lifecycle tests against the installed service.
4. Create `docs/test-reports/N1.md` … `N6.md` for the corresponding **future**
   checkpoint, with commands, environment, counts, failures/skips, measurements,
   limitations and acceptance IDs. The N prefix keeps M/C historical reports intact.
5. Commit report/docs, show `git log --oneline` for new commits and final status,
   identify intentionally uncommitted files, and stop awaiting the next milestone.

Capture Branching event/geometry/pixel fixtures and release benchmarks from a
clean baseline checkout before changing algorithms. Do not modify a v0.2.0 tag or
overwrite its release binaries/assets. Baseline outputs belong in a distinct
temporary/artifact directory. Preserve existing tests rather than rewriting
their expectations to fit the new mode.

## N1 — Configuration model and layout selection seam

**Status:** **Completed (2026-10-25; see [`docs/test-reports/N1.md`](../../test-reports/N1.md)). Stopped awaiting explicit N2 authorization.**  
**Dependency:** owner approval of this package and explicit N1 authorization.

Expected files:

- `apps/radivine/src/config.rs`.
- `crates/radivine-wheel/src/{config,lib,session}.rs`.
- Existing config tests and `crates/radivine-wheel/tests/unit_tests.rs`;
  new `apps/radivine/tests/nested_config.rs` if tables outgrow module tests.
- `docs/test-reports/N1.md` and implementation progress annotations in this package.

Commit units:

1. `feat(config): add backward-compatible wheel layout preference`: exact P4
   schema, tolerant field deserialization, warning/reset only that field, strict
   validation, pure enum mapping. Add malformed-type/string and legacy fixture
   tests with nondefault safety/trigger/preview/jobs/unknown keys.
2. `refactor(wheel): introduce explicit layout selection state`: add enum/default
   and requested/effective selection query; isolate layout-dependent entry points
   without changing Branching math, timings or state transitions. Keep shared
   lifecycle and tree/path ownership. Do not create a dynamic strategy trait.
3. `test(wheel): lock branching behavior before nested integration`: deterministic
   baseline trace assertions for slop, bridge, defaults, hysteresis, child overlap,
   hover intent, keyboard, enablement and interrupted animation.

The intermediate checkpoint accepts/stores the new config but is **not releasable
as functional Concentric**. Do not expose the Control Center selector or start
an unfinished desktop build. Report this explicitly; do not claim N1 implements
the feature. N2/N3 will complete the effective Concentric path before N4 exposure.

Tests/gate: T-CFG1–4 and T-REG1, existing wheel tests, app config mapping, fmt/lint.
No new dependency; no allocations in Branching hot paths from enum dispatch.
Baseline engine differential traces must match. Main risk is broad refactoring
of session state; rollback is reverting the focused unshipped seam commit via
a new commit. Complete when default/invalid/missing behavior and unchanged
Branching traces are documented and all checkpoint checks pass.

## N2 — Pure concentric geometry and hierarchy navigation

**Status:** **Completed (2026-10-25; see [`docs/test-reports/N2.md`](../../test-reports/N2.md)). Stopped awaiting explicit N3 authorization.**  
**Dependency:** N1 accepted; explicit N2 authorization.

Expected files:

- New `crates/radivine-wheel/src/{concentric,concentric_nav}.rs`.
- `crates/radivine-wheel/src/{lib,session,config,geometry}.rs`; `hit.rs` only for
  documentation/shared value types, not changing Branching hit rules.
- New `crates/radivine-wheel/tests/{concentric_geometry,concentric_scenarios}.rs`;
  extend `properties.rs`, `alloc_test.rs`, `benches/wheel_bench.rs`.
- `docs/test-reports/N2.md`.

Commit units:

1. `feat(wheel): add deterministic concentric preflight and annular geometry`:
   implement I1/I2 formulas, target core check, full configured-depth reservation,
   fixed caption box, anchor approach guard, reasons for startup fallback. Root
   placeholder and ready menu share the same center/plan. Test exact fit ±epsilon.
2. `feat(wheel): implement strict annular hit testing and eligibility`: I3/I4
   `NestedHit`, neutral separators/radial boundaries, independent hover hysteresis,
   fresh-motion guard and stable `ItemPath` mapping. No nested hit-margin overlap.
3. `feat(wheel): add concentric hierarchy navigation and safe activation`: I5–I7
   dwell/band/sibling adoption/hub transitions, default-child rules, pointer and
   keyboard takeover, generation-aware presses, timed enablement updates.
4. `test(wheel): cover nested timing boundaries and allocation invariants`:
   property/scenario suite, fixed RNG seeds and minimized failures, bounded
   retracting state; performance smoke measurements separate from full gates.

Required state transitions: Approach→Ready; deepest hover→Dwell→OpenChild;
ancestor sibling→Pending→Adopt; Hub→RootOnly; disabled ancestor→Reject; new
geometry→Unarmed→FreshMotionArmed; pointer/keyboard takeover; shared phase
Appearing/Open→Committing/Dismissing→Finished. `tick` never commits.

Tests/gate: T-GEO1–7, T-NAV1–10, T-SAFE1–5, T-MENU1–3 and wheel allocation tests.
Animation-specific T-NAV11–12 complete at N3; N2 still verifies common phase and
single-Finished invariants with its intermediate scene output.
Exercise 1/2/3/4/6 levels and 1..12 items, and disabled/deep More… nodes. Bound
state to six levels, twelve segments, one ghost per slot; zero steady-state
engine allocations. Branching traces unchanged and engine mean/percentile
measurements recorded. Risk: stale timer/path identity or invisible new targets;
rollback removes Concentric routing while retaining Branching, without altering
user config. Complete at pure-engine acceptance; no desktop sign-off implied.

## N3 — Scene, rendering and animation

**Status:** **Completed (2026-10-25; see [`docs/test-reports/N3.md`](../../test-reports/N3.md)). Stopped awaiting explicit N4 authorization.**  
**Dependency:** N2 accepted; explicit N3 authorization.

Expected files:

- `crates/radivine-wheel/src/{scene,session,anim,concentric}.rs`.
- `crates/radivine-render/src/{lib,damage,cache,theme}.rs` (cache changes only if
  needed for bounded nested text handling).
- `crates/radivine-render/tests/render_tests.rs`; new `concentric_render.rs`,
  test-owned goldens under `crates/radivine-render/tests/golden/nested/` and
  immutable Branching regression fixtures under a separate `golden/branching/`.
- `crates/radivine-render/benches/render_bench.rs`,
  `tools/wheel-sandbox/src/main.rs`; update all actual `WheelVisual`/`CaptionVisual`
  literal constructors found by `rg` rather than assuming this list is exhaustive.
- `docs/test-reports/N3.md`.

Commit units:

1. `feat(render): render explicit concentric annuli and path captions`: typed
   visual kind, one hub, no child disc shadows, supplied geometry/content boxes,
   breadcrumb, guide, no stems/pop. Match I8. Preserve Branching pixel output.
2. `fix(render): track concentric bounds and scene changes in damage`: hash all
   new draw inputs; analytic bounds and content extents; old∪new clearing on
   removal/replacement; full/incremental equivalence tests.
3. `feat(wheel): animate concentric visibility without moving targets`: opacity
   expansion/retraction, ghost replacement cap, interruption, reduced motion and
   all relevant deadlines. No action delay to show a flash.
4. `test(render): add nested visual fixtures and sandbox scenarios`: sandbox
   explicit layout selection and export to a supplied output directory; stable
   1/2/3/4/6-level, boundary/disabled/long-label frames, dark/light and scale cases.

Tests/gate: T-REN1–7, T-NAV10–12, T-PERF1–4 at reference resolutions. Human review
of generated screenshots is recorded as developer review, not owner desktop
acceptance. Measure engine and renderer warm allocations separately. Risk: child
hub occludes ancestors or full disc shadow dirties the cluster; assertions must
detect both. Rollback keeps Branching renderer path intact and removes nested
visual routing. Complete when visuals, damage correctness and preliminary
budgets pass; selector remains unexposed until integration safeguards are ready.

## N4 — Control Center, reload and X11 session integration

**Status:** **Completed (2026-10-25; see [`docs/test-reports/N4.md`](../../test-reports/N4.md)). Stopped awaiting explicit N5 authorization.**  
**Dependency:** N3 accepted; explicit N4 authorization.

Expected files:

- `apps/radivine/src/{config,session,ipc,main}.rs`.
- `apps/radivine-control/src/{persistence,service,ui/mod}.rs`.
- `packaging/config.example.toml` (new key only; no version/release change yet).
- New `apps/radivine/tests/nested_acceptance.rs` and
  `apps/radivine-control/tests/nested_settings.rs`; extend relevant M3/C3 reload
  and deferred-payload fixtures, using isolated sources/temporary files.
- `crates/radivine-platform/tests/x11_tests.rs` for reserved-disc and capture
  assertions. Production platform changes are expected only if a thin graphical
  state query is required; keep geometry out of platform and XDND code unchanged.
- `docs/test-reports/N4.md`.

Commit units (order is mandatory):

1. `fix(session): snapshot settings and action registry during wheel sessions`:
   active settings/registry, consistent MenuLimits, timed enablement, proposed
   pending selection by action ID, validation before successful finish/dispatch.
   Audit every active `self.config`/`self.registry` read; test repeated reloads.
2. `feat(session): integrate nested capture and display-change guards`: planned
   layout before map, no-compositor fallback, solid reserved-disc input union,
   no shrink while navigating, deferred scale and reject-until-drag-end behavior
   for nested monitor/compositor changes. Verify status before render.
3. `feat(control): add wheel layout selector with atomic persistence`: UI text,
   canonical values, Branching-only sliders, restore default, raw template/key,
   existing save pipeline, saved-vs-active status and honest reload messages.
4. `test(session): verify nested reload and action identity end to end`: both
   layouts, same leaf action IDs/context, no changes to presets/jobs, delayed
   payload identity, no-motion release, capture through layout saves.

Tests/gate: T-CFG1–8, T-X111–8, T-MENU1–5, T-SAFE1–8; unchanged M2/M3/M4/C3,
v0.1.1 persistence and C4 acceptance. Run the X11 subset on verified Xvfb; record
actual executed cases, not just process exit status. UI integration may be
exercised in headless egui. No installed service changes. Performance: no layout
rebuild per frame, no extra monitor surface, no worker dependency. Snapshot memory
and lifetime must be bounded and released at session end. Risk: reload releasing
capture or action identity mismatch; these are release blockers with no waiver.
Rollback preference to Branching **next session**; never hot-switch an active drag.
Complete when both configured modes can safely operate end to end in isolation.

## N5 — Interaction, regression and owner acceptance

**Dependency:** N4 accepted; explicit N5 and real-desktop verification authorization.

Expected files:

- All new/extended test and benchmark modules listed above; existing
  `apps/radivine/tests/{m6_soak_test,m6_fuzz_suite,c3_convert_acceptance}.rs`
  only for layout parameterization where appropriate.
- `.github/workflows/ci.yml` to run the actual isolated Xvfb subset if added;
  do not reference nonexistent features or scripts.
- `docs/test-reports/N5.md`, `docs/BENCHMARKS.md` (append new results), and
  `docs/test-reports/screenshots/nested/` for reviewed evidence.

Commit units: focused interaction fixes with reproducing tests; visual fixture
review; performance tuning confined to measured bottlenecks; benchmark and soak
reports; owner acceptance evidence. Do not bundle unrelated refactors.

Execute every mandatory test matrix row. Run all legacy workspace suites,
isolated Xvfb suites, C1–C5 suites, one-hour mixed-layout wheel soak and C5 job
soak, legacy ignored M6 fuzz runs and deterministic randomized nested scenarios.
Perform the real Mint Cinnamon X11 checklist with disposable file fixtures and
explicit source-location/inode/mtime/hash comparisons. Capture both layouts at
the same contexts, root counts and scales. Show the owner the fallback message,
edge-origin approach and long-label behavior.

Complete only with performance gates met, zero unintended actions/native moves
in claimed capture zones, no Branching visual/behavior regression and explicit
owner sign-off. If hardware/manual cases are unavailable, mark incomplete; never
substitute Xvfb success for owner acceptance. Risk: subjective navigation feel or
unmeasured raster cost. Tune only values approved in a documented plan revision;
do not add scrolling or alter fallback ad hoc. Branching remains the rollback.

## N6 — Packaging, docs and v0.3.0 preparation

**Dependency:** N5 accepted; explicit N6 authorization. No publication implied.

Expected files:

- `Cargo.toml` workspace version and `Cargo.lock` package-version records only;
  `packaging/debian/changelog`, `scripts/build-deb.sh`, `.github/workflows/ci.yml`
  versioned artifact names; inspect other hardcoded v0.2.0 references before edits.
- `packaging/config.example.toml`, `README.md`, `CHANGELOG.md`, new
  `RELEASE_NOTES_v0.3.0.md` and `docs/RELEASE_NOTES_v0.3.0.md` if retaining the
  repository's existing two-location release-notes convention.
- `docs/{PRODUCT_VISION,ARCHITECTURE,WHEEL_ENGINE_DESIGN,CONTROL_CENTER_DESIGN,PLATFORM_INTEGRATION,TESTING_STRATEGY,DEVELOPMENT,DECISIONS}.md`,
  `docs/test-reports/N6.md`. Manual pages are generated by the four MAN heredocs
  in `scripts/build-deb.sh`; update those sources, not generated `.1.gz` files.

Commit units: docs explaining optional layout/fallback/platform limits; version
and package preparation; isolated package upgrade verification; final report.
Add ADR-NW-001 to DECISIONS under available ADR-045 **only when owner acceptance
has occurred**; preserve ADR-010's historical text/status. If acceptance has not
occurred, keep the ADR Proposed and do not describe the feature as accepted.

Run full checkpoint suite, dependency graph, audit/deny, package build/lintian,
clean install and upgrade **from authentic v0.2.0** in isolated containers/VMs.
Verify missing layout remains Branching, explicit preference persists, custom
actions/comments/presets/jobs survive, and uninstall preserves user data. Do not
install over the owner's running package. Record sizes and compare budgets.

Complete with reviewable v0.3.0 release candidate documentation/package evidence
and clean Git status. No push, tag, release, upload or replacement of v0.2.0
assets. Publication is a separate owner instruction. Rollback is retaining the
published v0.2.0 package and selecting Branching in the development candidate.
