# Nested wheels architecture and repository findings

Status: **Proposed; no implementation in this planning change.**
Baseline: `e6a342e`, 2026-10-25. Paths below are repository-relative. New module and
type names are explicitly proposals, not claims that they already exist.

## A1. Verified implementation map

| Existing source | Actual responsibility / relevant symbols |
|---|---|
| `crates/radivine-wheel/src/config.rs` | `WheelConfig`, `validate`, `SubmenuOpen`, `InvokeSelect`; runtime dependency is only `smallvec`. |
| `crates/radivine-wheel/src/menu.rs` | `MenuTree`, `MenuNode`, `NodeKind`, `ItemPath(SmallVec<[u8;6]>)`, `sanitize`; More… and depth bounds. |
| `crates/radivine-wheel/src/geometry.rs`, `layout.rs` | Root/child spans, polar math, corridors; `place_root`, `place_child`, `nominal_radii`, `place_caption`. |
| `crates/radivine-wheel/src/hit.rs` | `WheelHitGeometry`, `Zone`, `hit_test`; deepest first, hub before annulus, angular hysteresis, parent-overlap guard. This cannot be reused unchanged for shared centers. |
| `crates/radivine-wheel/src/session.rs` | `WheelSession`, private `WheelInstance`, `open_child_wheel`, `truncate_after`, `handle_pointer_moved`, `handle_pointer_release`, `handle_nav_key`, `drop_target`, `set_menu`, `set_enabled`, `rebuild_scene`, `rebuild_input_region`. One open chain plus retracting snapshots. |
| `crates/radivine-wheel/src/anim.rs`, `scene.rs` | Injected `Millis`, deterministic `Tween`, timings; `Scene`, `WheelVisual`, `SegmentVisual`, `CaptionVisual`, `Shape`. |
| `crates/radivine-render/src/lib.rs` | `Renderer::render`, `draw_wheel`, `draw_segment`, `draw_hub`, `draw_caption`. Each wheel currently draws a disc shadow and hub; gap is hardcoded to 2.5 lp. |
| `crates/radivine-render/src/damage.rs` | `DamageTracker::compute_damage`, base/segment hashes, bounds, `merge_into_at_most_4`; segment bounds sample seven angles. |
| `crates/radivine-render/src/cache.rs`, `theme.rs` | Cached glyph/icon/shadow masks and theme tokens; keep platform-independent and `forbid(unsafe_code)`. |
| `crates/radivine-core/src/registry.rs` | `ActionRegistry::build_menu`, `MenuLimits`, `ActionIndex`, `enablement_updates`; builds index after visibility, grouping and overflow. |
| `apps/radivine/src/config.rs` | `WheelSection`, `AppConfig::from_toml_str`, `sanitize`, `validate_strict`, `to_wheel_config`, `to_x11_config`. |
| `apps/radivine/src/main.rs`, `ipc.rs` | Reload IPC calls `SessionController::reload_all`; status uses `DaemonStatus`. There is no separate layout IPC. |
| `apps/radivine/src/session.rs` | `summon_drag` creates menu-less placeholder; `summon_invoke` builds menu first. `handle_uri_list_payload`, `activate_provisional_file_menu`, `handle_dnd_drop`, `commit_path`, `apply_enrichment_context`, `sync_dnd_status_if_changed`, `sync_input_region`, `render_and_present`. |
| `crates/radivine-platform/src/x11/{mod,overlay,shape,monitors,xdnd}.rs` | One monitor overlay, XShape mask conversion, Xft scale, XDND Private/None replies, preparation/map/nudge, presentation. Platform owns no menu geometry. |
| `apps/radivine-control/src/ui/mod.rs` | `render_wheel_page`, `save_config`, `WorkerCommand::ReloadDaemon`; saved/draft config and jobs settings. |
| `apps/radivine-control/src/persistence.rs` | `DEFAULT_CONFIG_TOML_TEMPLATE`, `apply_app_config_to_toml_doc`, `apply_app_and_jobs_config_to_toml_doc`, `DiskFileSnapshot`, `atomic_write_with_backup`. |
| `apps/radivine-control/src/service.rs` | `ServiceController::reload_daemon` sends existing IPC; no restart required. |
| `tools/wheel-sandbox/src/main.rs` | Engine/render demo, deterministic screenshots, debug overlay; extend this tool rather than add another UI framework. |

Required design sources were reviewed: `AGENTS.md`, PRODUCT_VISION, DECISIONS,
ARCHITECTURE, WHEEL_ENGINE_DESIGN, CONTROL_CENTER_DESIGN, PLATFORM_INTEGRATION,
TESTING_STRATEGY, BENCHMARKS and DEVELOPMENT under `docs/`, along with M1a–M6 and
C1–C5 reports and the corresponding tests. Source discrepancies matter:

- App defaults are root 40/128, child 34/108, offset 20, dwell 110 ms, grace
  180 ms, hysteresis 3.5°. Pure engine defaults remain 38/116, 30/96, offset 10,
  dwell 140, grace 300, hysteresis 4°. Use app defaults in desktop acceptance.
- Runtime pass-through key is `[input].passthrough`, despite ADR-016's older
  `wheel.leave.passthrough` spelling. Its default is false. PRODUCT_VISION's
  general “outside passes through” sentence is not the current default policy.
- Reload does **not** dismiss the session. `reload_config` replaces shared
  `config`/`wheel_cfg`; `reload_all` also replaces `registry`. The existing engine
  owns its config copy, but app input policy reads shared config and action
  dispatch reads shared registry. Layout switching needs explicit session
  snapshots so these indirect effects cannot change an active drag.
- `set_menu` is one-shot before hover, not a live structural reconciliation API.
  `set_enabled` mutates flags/reasons only. Its rebuild timestamp currently comes
  from the root tween's start; use an explicit timestamp for new nested updates.
- Provisional drop currently carries only an `ItemPath` while payload handling
  can rebuild `ActionIndex`. Pin action identity for this path; never infer that
  an old numeric path identifies the same action in a different real context.
- `SettingsChanged | MonitorsChanged` currently calls `prepare_overlay` and
  restarts renderer buffers while retaining wheel geometry; the older platform
  document's claim that these events dismiss the session is not the code.
- Renderer “snapshot” tests check pixel counts and same-process determinism;
  they do not compare checked-in golden files. `tests/e2e-x11/`, the documented
  separate x11/nightly workflows, and `tools/convert-fuzz` are absent at baseline.
  Actual fuzz targets are in `fuzz/fuzz_targets/`; actual CI is `.github/workflows/ci.yml`.

Do not edit historical accepted documents during planning to hide these differences.

## A2. Architecture choice

| Approach | Assessment |
|---|---|
| `dyn WheelLayoutStrategy` with placement/hit/render/navigation hooks | Two closed layouts do not justify vtables, trait-object storage, lifetime complexity or cross-hook invariants. Geometry alone cannot express different hierarchy guards. Reject. |
| Separate complete engines behind an enum | Preserves old code but duplicates menu ownership, lifecycle, keyboard dispatch, timers and enablement. Likely drift in safety behavior. Reject. |
| Enum plus pure nested geometry and a small nested navigation state | Chosen. Keep common menu/path/lifecycle/event contract and existing Branching algorithm; dispatch only genuinely different geometry/input/scene construction. |

Add `WheelLayout { Branching, Concentric }` in wheel `config.rs`, default Branching,
with no serde dependency. Add `layout: WheelLayout` to `WheelConfig`. Add private
`LayoutState { Branching, Concentric(ConcentricState) }` to `WheelSession`. The
requested/effective layout and fallback reason are separately queryable. Branching
should execute the existing code paths, with only an enum branch at entry points.
Do not parameterize vine corridors to approximate rings.

Proposed new files:

- `crates/radivine-wheel/src/concentric.rs`: pure preflight, annuli, sectors,
  content boxes, strict hit test and hover hysteresis helper.
- `crates/radivine-wheel/src/concentric_nav.rs`: nested-specific guard/timer data
  and transition decisions. Keep field access private to the crate. No I/O,
  clocks, action registry, serde, trait objects, or renderer types.

Use existing `WheelInstance` chain for identity, segments, tweens and role. Nested
instances have identical center, increasing radii, no bridge, no stem, scale 1.
Keep nested geometry in `ConcentricState`; do not route it through Branching's
`hit_test`. `WheelHitGeometry` may still serve debug/item-center queries but must
be documented as a representation, not a usable nested hit-test function.

## A3. Proposed types and integration seams

The following interfaces are the implementation contract; visibility may be
`pub(crate)` except existing public APIs and test queries:

```rust
enum LayoutFallback { InsufficientSpace, TargetTooSmall, NoCompositor }
struct LayoutSelection {
    requested: WheelLayout,
    effective: WheelLayout,
    fallback: Option<LayoutFallback>,
}
struct RingGeometry {
    level: u8,
    center: Point,
    r_in: f32,
    r_out: f32,
    separator_half_angle: f32,
    spans: SmallVec<[SegmentSpan; 12]>,
    content_boxes: SmallVec<[Rect; 12]>,
}
struct ConcentricPlan {
    center: Point,
    reserved_radius: f32,
    caption_box: Rect,
    radii: SmallVec<[(f32, f32); 6]>,
    approach_required: bool,
}
fn plan_concentric(anchor: Point, viewport: Viewport, cfg: &WheelConfig)
    -> Result<ConcentricPlan, LayoutFallback>;
fn ring_geometry(plan: &ConcentricPlan, level: u8, count: usize) -> RingGeometry;
fn hit_concentric(p: Point, plan: &ConcentricPlan, open: &[RingGeometry]) -> NestedHit;
```

`ConcentricState` stores the plan, open ring geometries, strict hit, pending sibling
`{path, entered_at, deadline}`, pointer exposure guards per level, approach state,
keyboard/pointer source and takeover point, plus last injected timestamp.
It also stores `interaction_suspended`, initially false, for the app's irreversible
reject-until-drag-end guard; this is independent of the animation/session phase.
Timers refer to paths and a monotonically increasing geometry generation; verify
both before acting. No geometry stored in `ActionIndex`. Bound collections to six
levels/twelve items; reject nonfinite geometry before any trigonometry or indexing.

Integration in `WheelSession`:

| Existing operation | Nested-specific implementation |
|---|---|
| `new` | Validate, preflight, select effective layout before scene/input output; allocate bounded state; create root or placeholder. |
| `set_menu` | Keep one-shot rule; create root spans without moving center; install pointer exposure guard. |
| New `set_menu_at_pointer(menu, pointer, now)` | Explicit one-shot installation with the host's last observed position. Nested saves that point before installing exposure guards, without treating it as motion. Existing `set_menu` delegates using its last known pointer; Branching behavior is unchanged. |
| `handle_pointer_moved` | Dispatch to nested hit/guard/transition path or unchanged Branching path. |
| `open_child_wheel`, `truncate_after` | Share path/events/lifecycle bookkeeping; nested constructs/drops annuli and retires at most one ghost per slot. |
| `tick`, `next_deadline` | Include sibling adoption and guards; injected time only; no frame-driven selection. |
| `handle_pointer_release`, `drop_target`, `commit_drop` | Nested strict eligibility; old Branching rules retained. |
| `handle_nav_key` | Common navigation semantics with explicit input-source tracking for nested; disabled focus retained. |
| `set_enabled` | Keep compatibility entry point. Add `set_enabled_at(path, enabled, reason, now)` for callers with a clock; nested uses now and clears invalid timers/eligibility; legacy behavior remains delegated to its current logic. |
| New `suspend_interaction(now)` | Nested only: set the suspension flag, clear hover/press/armed targets and navigation timers, preserve scene geometry and input region. Further motion/ticks cannot arm or commit. No Finished event or automatic unmap. No resume within this session. |
| `scene`, `input_region` | Existing borrowed output APIs remain. No allocation of a new scene on every query. |
| `debug_state`, `item_center`, `wheel_geometry` | Preserve Branching test API; add nested diagnostics containing strict hit, guards, fallback and radii. |

The app should pass consistent `MenuLimits` derived from wheel max-items/depth
when building both normal/provisional menus, rather than creating a differently
sanitized tree after `ActionIndex` construction. Current app values remain 8/4.
Assert path→ID parity before and after wheel sanitization. No change to action
visibility or depth-limit policy is part of this feature.

## A4. Scene and renderer contract

Retain `Scene.wheels` and reuse `SegmentVisual` action metadata. Add proposed:

```rust
enum WheelVisualKind { Branching, ConcentricRoot, ConcentricRing }
enum SelectionVisual { None, Pending, Armed, KeyboardFocus }
enum ContentMode { IconAndLabel, IconOnly, LabelOnly }
struct NestedSegmentContent { rect: Rect, mode: ContentMode }
struct OriginGuideVisual { from: Point, to: Point, opacity: f32 }
// WheelVisual additions:
//   kind: WheelVisualKind
//   nested_content: Option<SmallVec<[NestedSegmentContent; 12]>>
//   separator_half_angle: Option<f32>
// SegmentVisual addition:
//   selection: SelectionVisual (Branching ignores it and supplies None)
// CaptionVisual addition:
//   breadcrumb: Option<SmallVec<[Arc<str>; 6]>>
// Scene addition:
//   origin_guide: Option<OriginGuideVisual> (neutral approach line)
```

`Branching` values use `None` additions, preserving existing rendering: hub,
shadows, 2.5 lp gap behavior, pop and stems. All existing literal constructors,
benchmarks and sandbox code must be updated explicitly. Do not accidentally
infer kind from `r_in`, `r_out` or the absence of a bridge.

`ConcentricRoot` draws the single hub. `ConcentricRing` draws **annular material
only**, never a hub or disc shadow. Both nested kinds have scale 1 and pop 0,
use supplied sector inset/content boxes and the I8 opacity rules. Renderer
consumes boxes and exact spans; it must not independently place a wheel. Origin
guide is underneath ring material, never an input item. Breadcrumbs contain
ancestor label Arc clones in inline storage, not a newly joined string. Renderer
uses cached label and separator glyph runs, eliding runs to the caption width;
do not precompute every possible path string in a large tree. Caption
wrapping/truncation is renderer-owned.

No renderer change to the platform, pixel format or SHM protocol. Reuse bundled
font/icons, dark/light theme, missing-icon behavior. No new font/network/asset
dependency. Any additional nested outline token belongs in `theme.rs`; do not
touch `assets/branding/` or `assets/icons/`.

## A5. Damage, caches and budgets

Extend `damage.rs` hashes for kind, separator inset, content box/mode, breadcrumb,
origin guide, and any new outline state. Include all changed draw parameters.
For nested sectors, replace sampled-angle bounds with conservative analytic
bounds: endpoints at both radii plus each cardinal angle inside the span,
union content box, rim marker and stroke/AA padding. Treat one-item annuli as
full discs for bounding purposes. Preserve Branching bounds initially to avoid
unrelated visual changes; a common conservative fix requires differential tests.

Maintain old∪new damage for removed/replaced rings. Same slot replacement may
reuse an element key only if hash/bounds detect every visual difference. Clip
redraw of **all** overlapping elements to damage, not only the changed element.
Fresh full render must equal the incremental render byte for byte. At most four
presentation rectangles; no whole-monitor upload merely because a ring changed.
Worst-case nested bounding square is larger than three small vine wheels; do not
pretend its raster cost is constant. Avoid per-ring blur caches and animated
radius cache churn. Existing font caches remain; bound any new per-session cache.

Budget details and measurement rules are normative in [TEST_MATRIX.md](TEST_MATRIX.md#t6-performance-and-memory-gates).
No runtime dependency additions. Nested engine incremental state target ≤64 KiB
per session excluding the existing menu/labels; no additional monitor framebuffer.
Allocation gate is zero steady-state engine allocations, including repeated
open/close after menu preparation with caller-preallocated events. Renderer warm
allocations are measured separately; do not claim existing renderer zero-allocation
coverage from the wheel allocator test. New warm nested paths must allocate zero.

## A6. Config persistence and daemon snapshots

Use app-owned `WheelSection.layout: String`, default `"branching"`, to match other
lenient app preferences. Give this field a custom deserializer that reads a
`toml::Value`: strings are preserved for validation; any other type becomes an
invalid sentinel string. `sanitize` warns and resets this field alone. Strict
validation accepts exactly `branching|concentric` without case folding or aliases.
`to_wheel_config` maps to the pure enum. The UI operates on canonical strings and
never writes the invalid sentinel. This avoids adding serde to wheel or allowing
an unknown enum variant to invalidate unrelated settings. Test direct strict
deserialization as well as runtime sanitize.

Add the key to `apply_app_config_to_toml_doc`, its default template, and eventually
`packaging/config.example.toml`. Reuse `DocumentMut`, complete-candidate validation,
conflict snapshots and `atomic_write_with_backup`; do not serialize an entirely
new config over the original. The wrapper that also updates `[jobs]` remains the
save path. `convert.toml` and `actions.toml` must not be rewritten by a layout save.
Current save also sends worker ReloadSettings; retain its idempotent behavior,
and assert no job cancellation or changed jobs settings. No new worker command.

Add app-owned `ActiveSessionSettings` containing a clone of `AppConfig`, effective
scale/theme/reduced motion, and `Arc<ActionRegistry>`, plus layout selection.
Capture it before initial menu/placeholder construction. The clone is bounded
session setup work, not hot-path work. Continue updating global config/registry
for the next session on reload. Every active-session read of input policy,
leave/toggle behavior, menu construction, enrichment, action lookup and dispatch
uses this snapshot; audit `self.config` and `self.registry` reads in `session.rs`.
Registry snapshot is especially necessary because even a layout-only save calls
`load_actions_from_path`. Existing background jobs own their context independently.

For initial real or provisional Drag menus, call `set_menu_at_pointer` with the
converted `last_dnd_pos` (or summon point if none), then preserve the existing
position replay. The replay is not a new physical observation and cannot clear
the exposure guard. This is necessary because positions received while Summoning
need not already have been delivered to the engine.

Keep action index and context session-scoped. Replace proposed pending field
`Option<ItemPath>` with `Option<PendingDropSelection { path, action_id }>`.
Capture ID before requesting a deferred real payload. On completion, look up
that ID in the rebuilt real-context index and check visibility/enablement against
the same registry snapshot. Missing/disabled means `dnd_finish(false)` and no
dispatch; never fall back to a numeric index. Refactor commit validation enough
to avoid `dnd_finish(true)` before this validation. Successful dispatch still uses
`Dispatcher`; no duplicated conversion/custom-command implementation.

Add optional serde-defaulted status fields in `ipc.rs`: `requested_layout`,
`active_layout`, `layout_fallback_reason`, `layout_pending` (true when a visible
session's snapshot differs from next-session preference). Existing status fields
and consumers continue to work; update test literals. Control Center save/reload
banners distinguish saved, reload acknowledged, deferred-until-next-activation,
and IPC failure. Do not introduce a service restart.

## A7. X11 safety and platform boundaries

Keep ADR-002's single monitor-sized overlay, `prepare_overlay` before map,
zero-pixel nudge, no Drag grabs, Private/None protocol replies and no DELETE.
On `DndPosition`, calculate/reply before rasterization. After timers and
enablement changes, call `sync_dnd_status_if_changed`; drop always rechecks.

For nested engine `input_region`, reserve one **solid disc** of radius
`Rreserve + cfg.hit_margin`, plus anchor disc and approach corridor for the entire
interactive session. Gaps, hub and unopened radial capacity remain captured
neutral space, never annular holes in XShape. This disc does not shrink when
rings close. It is independent of executable hit zones. In safe default Drag
and in Invoke, app still sets `ShapeInput = None` (full overlay); no extra XShape
mutation per animation frame. Explicit pass-through uses the reserved solid
union. Captions remain outside the engine's interactive shapes.

No compositor: choose Branching at app session creation before any mapping, and
record `NoCompositor`. Do not pass platform capabilities into pure wheel math;
the app overrides the effective request while preserving requested status. The
existing opaque fallback clips `ShapeBounding` to visible discs/caption, which
also limits effective input even with full ShapeInput. This is an existing
limitation and must not be described as screen-wide safe cancellation. Nested
release approval requires the composited Cinnamon path; opaque Branching behavior
must not regress. Cross-monitor native drop outside the selected monitor and
explicit pass-through are likewise existing limits, not guaranteed cancellation.

For an **active effective Concentric** session, defer Xft scale/reduced-motion
setting changes until its end; do not reprepare the overlay with old wheel
geometry. Snapshot monitor origin/size. If monitor geometry or compositor
availability changes during nested Drag, enter app-local `reject_until_drag_end`:
call `suspend_interaction`, send reject, retain the currently mapped full capture
window/region, and do not apply layout/config updates. Answer any delivered drop
with failure and hide after source release/owner loss. Explicit Shift toggle
remains deliberate native handoff. Invoke may cancel immediately and ungrab.
Handle this guard before generic dismiss/timer routing: do not call engine
`dismiss`, clear XShape, or act on a normal inactivity timeout while the source
still owns the held drag. Source-disarmed/released events and explicit user
handoff terminate the guard. Keep the armed-source event observation active.
Once back to Idle/Armed, synchronize graphical state and prepare normally next
time. Do not restart rendering with mixed old/new coordinate transforms.

This guard is confined to nested sessions; do not refactor legacy display-event
handling wholesale. A monitor removal/X-server loss can make continued capture
impossible; never synthesize input or claim source protection after losing the
target window. Test graceful recovery/no dispatch and document that external
failure boundary. No additional windows or virtual-desktop capture are proposed.

## A8. Decisions and delivery boundaries

The new [ADR-NW-001](ADR-NW-001.md) is proposed for eventual registry number ADR-045
(verify number availability when implementing). ADR-010 remains Accepted for
Branching. ADR-006, ADR-009, ADR-012 and runtime ADR-016 policy remain invariants.

The milestone plan names exact expected files and checkpoints. Changes to
conversion codecs, worker protocol/store, background job logic, viewer behavior,
custom action semantics or protected assets are outside scope. A discovered
unrelated baseline bug is documented separately; do not use this feature to
rewrite the application. Required session identity/capture guards above are
direct consequences of safely supporting live layout preference changes.
