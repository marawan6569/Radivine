# Gemini implementation handoff — Concentric / Nested Wheels

**Read first: this package is planning, not permission to implement.**
The owner must authorize a named N milestone. Stop at its checkpoint after tests,
report and commits. Do not begin the next milestone merely because it is listed
here. Do not modify the installed application, real user config/data, Nemo
settings, or services without authorization for those operational effects.

Baseline investigated: **v0.2.0**, Git **`e6a342e`**, 2026-10-25. The repository
was clean at the start; prior C5/release documentation was already committed.
The M6-era state in AGENTS.md is historical, not an invitation to repeat M/C work.
Keep v0.2.0 public history, tags, release notes/assets and packages intact.

## Reading order and authority

1. Repository `AGENTS.md` and the owner's latest authorization.
2. [PRODUCT_SPEC.md](PRODUCT_SPEC.md): observable behavior, schema, defaults,
   selector wording, next-activation behavior and fallback policy.
3. [INTERACTION_DESIGN.md](INTERACTION_DESIGN.md): exact math, boundaries,
   hover/navigation, activation, cancellation and visual rules.
4. [ARCHITECTURE.md](ARCHITECTURE.md): verified source map, proposed types/modules,
   scene/renderer changes, config snapshots, stable action identity and X11 guards.
5. [IMPLEMENTATION_PLAN.md](IMPLEMENTATION_PLAN.md): N1–N6 work and stop points.
6. [TEST_MATRIX.md](TEST_MATRIX.md): acceptance IDs, real suite paths, isolation,
   metrics and mandatory owner desktop checklist.
7. [ADR-NW-001.md](ADR-NW-001.md): Proposed alternative to ADR-010, not its replacement.

If source evolves after this baseline, inspect the diff and reconcile the plan
before coding. Do not silently replace these decisions with a generic radial
menu implementation. In a conflict, the owner's explicit instruction wins; a
technical contradiction must be reported and the affected plan corrected before
dependent work. Do not use obsolete examples in older design docs over verified
runtime behavior.

## Fixed decisions — do not redesign

| Topic | Implement this |
|---|---|
| Default | Branching / Vine for both new and old users. Preserve its algorithms and visuals. |
| Selection | `[wheel] layout = "branching"` or `"concentric"`; exact strings; missing/invalid→Branching with field-local warning for invalid values. |
| Engine organization | Pure enum dispatch plus `concentric.rs` and `concentric_nav.rs`; shared menu/path/lifecycle; no strategy trait object or duplicate action engine. |
| Child arrangement | Full circumference per submenu; equal sectors, index zero at top, clockwise, all centers equal. No parent-slice fanout. |
| Radii | Validated existing root radii; child thickness 60 lp, gap 6 lp. Reserve configured max depth and 280×64 caption before mapping. |
| Space | Clamp whole reserved cluster once; approach-to-hub guard when displaced. If it cannot fit/minimum targets fail, Branching for entire session. No scrolling, clipping, dynamic shrinking or recentering. |
| Path | One open child per ring; logical `ItemPath` independent of geometry; ancestors visible and path-marked. |
| Navigation | Existing dwell/band config; whole neutral gap plus sibling adoption grace instead of vine triangle. Hub timeout returns to root. I5 is the transition table. |
| Release | Strict material hit independent of hysteresis. No stale leaf/default in gaps; new geometry requires subsequent ≥2 lp pointer motion. Timers never Commit. |
| Invoke | Existing click/release and complete keyboard navigation, disabled focus reasons, explicit keyboard/pointer takeover. |
| Rendering | Explicit Branching/ConcentricRoot/ConcentricRing visual kinds; one hub; no nested disc shadows, stems or radial pop; opacity-only expansion/retraction. |
| Availability | Menu structure frozen after initial install; enablement/reasons update in place; ancestors checked for eligibility. |
| Reload | Existing atomic save + Reload IPC; pin active config, capture policy and registry; latest saved/loaded preference takes effect next session. |
| Pending payload | Capture ActionId before deferred payload; remap by identity in real context and revalidate before success/dispatch. |
| X11 | Existing single monitor target, Private/None, no DELETE, no Drag grabs, zero-pixel nudge, full capture safe default. Nested pass-through shape is a solid reserved disc, never shrinking annular holes. |
| Platform limits | No-compositor preflight fallback; Xft scale is global; cross-monitor/native pass-through risks explicitly retained/documented. Nested display disruptions reject until drag end. |
| Dependencies/assets | wheel only smallvec at runtime; engine/render forbid unsafe; no protected branding/icon edits, no conversion/worker redesign. |
| Release | Prepare v0.3.0 only after N5 acceptance and N6 authorization. No tag/push/publication without separate instruction. |

## Practical implementation sequence

At N1, establish regression fixtures before extracting any method. Keep old
`hit_test`, placement and hover-intent code executable without changed arithmetic.
Map the app string into a pure wheel enum; do not add serde to `radivine-wheel`.
Unknown layout must not cause loss of unrelated config. This intermediate branch
is not a finished feature and must not be exposed as one.

At N2, implement/test preflight and strict annular hit testing first. Then add
motion/selection guards and timers. Use `ItemPath` in timer identities. Share
lifecycle and menu ownership; dispatch into nested-specific navigation before
Branching assumptions run. The deepest-wheel-first hub check is invalid for
nested geometry. Test both real app defaults and engine defaults.

At N3, add visual kinds before drawing rings. A concentric child cannot go
through unconditional `draw_hub` or disc-shadow drawing. Freeze all nested
geometric transforms and animate opacity only. Add exact bound/hash coverage
before tuning speed; compare incremental pixels to fresh-render pixels. Cache
content geometry and reusable raster/text work, retain small bounded state, and
measure warm allocations. Do not assume existing AlphaMask clones are cheap:
the baseline mask owns `Vec<u8>`. Prefer split borrowing/borrowed cached masks or
stable prepared handles for nested draws over copying mask bytes each frame.

At N4, land session snapshots and identity validation **before** the UI selector.
Audit every read of shared config/registry while a session exists. Ensure layout
reload cannot enable pass-through or switch the action object under a displayed
item. Distinguish desired and active layout in status. Integrate solid input
regions and display guards without moving geometry to the platform crate. Then
add the selector/persistence/help text and isolated end-to-end tests.

At N5, compare both modes against the same action/context fixtures; preserve
existing C1–C5 functionality. Run actual measured budgets and the owner checklist.
No claim of real Nemo acceptance from headless/Xvfb tests. Record missing display,
service or hardware coverage as incomplete. If UX revisions are requested,
update the plan/ADR/tests together and obtain scope authorization before adding
overflow features or other new modes.

At N6, update current docs and package metadata for v0.3.0, test upgrade from
authentic v0.2.0 in isolation, preserve all user data and history. ADR promotion
requires explicit owner acceptance. Prepare a reviewable release candidate;
publication is outside these milestones' default authorization.

## Review traps that must have explicit tests

- Same ItemPath may mean a different action after context/menu rebuilding.
- A visual “gap” in Branching is currently decorative; do not globally change
  old hit testing to apply nested gap rejection.
- Config reload changes shared app policy/registry even though the engine's
  copied config stays fixed. Saving only layout still calls action reload.
- Delayed menu installation can put a target beneath a stationary pointer;
  synthetic duplicate positions are not deliberate new movement.
- Keeping an ancestor highlighted for navigation must not keep its default
  action selectable in neutral zones.
- Drawing every ring's hub/shadow hides or darkens earlier rings. Scaling a ring
  about the center also overlaps ancestors. Both are forbidden for nested.
- Closing a ring must not shrink capture into a hole over a native Nemo target.
- XShape Bounding intersects effective input; `ShapeInput=None` is not a promise
  of monitor-wide capture in the old opaque fallback.
- A global Xft scale update cannot be applied to an active session's old logical
  coordinates. Do not reuse the existing reprepare path for nested sessions.
- Damage sampled at a few angles can under-bound wide sectors or horizontal text.
- Cached render masks contain heap-owned pixel data; warm clone/path construction
  allocations require measurement and targeted mitigation.
- The C5 soak default is six seconds; the M6 long soak/fuzz tests are ignored.
  A default workspace test run is not full-duration release verification.
- Current X11 tests self-gate on an actual isolated Xvfb PID; there is no daemon
  `x11-integration` feature or existing `tests/e2e-x11/run_e2e.sh`.

## Checkpoint report template

For each authorized N milestone, report:

1. Scope authorized and completed; remaining acceptance IDs.
2. Files and architectural changes, with any agreed plan revision.
3. Focused and full checks with counts and commands; executed versus skipped
   display/service/ignored cases. Link raw measurements and reviewed images.
4. Action safety, Branching differential results, resource/performance numbers.
5. Real owner acceptance status where required; never infer it from automation.
6. Conventional commits from `git log --oneline`; final status/uncommitted files.
7. Stop statement. Do not proceed to the next milestone without authorization.

Use reports `docs/test-reports/N*.md` during implementation. This planning task
creates no fake implementation report and makes no acceptance claims for a
feature that does not exist yet.

## Planning delivery and verification record

The seven requested documents in this directory form one proposal. Foundation
specification/architecture/ADR were committed as `82159c1`
(`docs(planning): specify nested wheel interaction and architecture`). The final
planning commit adds the sequence, matrix and this handoff, plus review corrections.
Use `git log --oneline -- docs/planning/nested-wheels` for its exact final hash.

Baseline read-only build diagnostics passed: formatting, workspace Clippy and
30 wheel/render tests; details and deliberate exclusions are in T8. No app source,
Cargo dependency, protected asset, existing release document, service, Nemo
setting or real user config/data was changed. No push/tag/release was performed.
Relative document links/anchors, radius/viewport arithmetic and the docs-only
Git diff are checked before the final planning commit.

Owner decisions outstanding are **proposal acceptance**, not missing algorithms:
accept the startup fallback/edge approach tradeoff and proposed budgets, then
authorize N1 if desired. Actual visual feel and timing acceptance occurs at N5.
If the owner requires Concentric on every small display instead of fallback,
stop and obtain a revised overflow product specification before implementation.
