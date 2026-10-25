# Concentric interaction contract

Status: **Proposed** · Baseline `e6a342e` · All numbers below are design targets,
not measurements. This file is authoritative for nested geometry and input.
Branching continues to use its existing implementation without these new rules.

## I1. Coordinates, annuli and sectors

Use existing `Point`, `Rect`, `Viewport`, `Millis`, `SegmentSpan`, and `ItemPath`.
All geometry is overlay-local logical pixels, finite `f32`; +y is down. Angles
increase clockwise, normalized to `[0, 2π)`. Index zero is centered at `3π/2`.
Depth `d` is zero-based; `D` is the number of possible rings including root.

Let the **validated engine** root radii be `h = cfg.radius_root[0]` and
`r0 = cfg.radius_root[1]`. Nested constants: thickness `T = 60`, ring gap `G = 6`,
sector separator width `S = 2.5`, target core `32 × 32`, edge pad `E = 8`.

```text
inner(0) = h                       outer(0) = r0
inner(d) = r0 + (d - 1)*(T+G) + G  outer(d) = r0 + d*(T+G), d >= 1
center(d) = C                     Rreserve = outer(D - 1)
Δ = 2π/n                          φ(k) = 3π/2 + k*Δ
a0(k) = φ(k) - Δ/2                a1(k) = φ(k) + Δ/2
```

Every ring uses its own `n`, in menu order, from 1 through 12. There is no bridge
sector, parent-relative angular rotation, or allocation weighted by descendants.
For `n = 1`, draw/hit a complete annulus without an artificial seam. For `n = 0`,
draw only the placeholder hub; an empty finalized menu follows existing no-actions
handling. Automatic More… and disabled-at-depth-limit nodes come from the common
menu sanitizer, not from geometry. Never repartition enabled items around a
disabled sector. App menu limits remain the current 8/4; engine tests cover 12/6.

Reject the nested plan if root thickness is less than 60 or any possible ring at
`n = cfg.max_items_per_wheel` cannot contain a 32×32 **radial/tangential** target
rectangle centered at `rc = inner + 0.55*(outer-inner)`. Test the rectangle's four
corners for radius bounds and separator-inset angular bounds; this is a minimum
core, not a promise that an entire 12-item wedge is 32 px wide at its inner tip.
Do not change `MenuTree` limits or user radii to pass this check.

Sector separators have a neutral angular half-width
`δ = asin(min(1, S/(2*inner)))`, constant across the annulus. Both renderer and hit
tester use the same stored inset. This is deliberately conservative at the outer
rim. For `n > 1`, selectable angles are strictly inside `(a0+δ, a1-δ)`.
The separator boundaries themselves are neutral. An endpoint within `1e-5` radians
of a separator edge is neutral; a radial boundary within `1e-4` lp is neutral.
No hit margin extends an executable sector into an adjacent ring or outside the
outer rim. `cfg.hit_margin` is capture/navigation padding only for Concentric.

## I2. Placement and overflow

Reserve **all `cfg.max_depth` rings**, regardless of currently open depth or menu
availability. This makes pre-payload Drag placement deterministic. Do not count
currently enabled items when planning capacity: enrichment cannot change fit.

Reserve a caption width of 280 and height of 64 below the maximum ring, separated
by 10. For viewport `(x,y,w,hv)`, the permissible center interval is:

```text
Cx ∈ [x + E + max(Rreserve, 140), x + w - E - max(Rreserve, 140)]
Cy ∈ [y + E + Rreserve, y + hv - E - Rreserve - 10 - 64]
C  = componentwise_clamp(activation_pointer, these intervals)
```

An empty interval or failed target check selects Branching **before any root is
shown**. No data-dependent fallback during navigation. The caption is always at
`(Cx-140, Cy+Rreserve+10, 280,64)`; reserve it even when no caption is drawn. Nested
uses no blurred disc shadows and no radial hover pop, so E covers strokes and
submenu markers. Examples with the shipped app root `40..128`:

| Capacity | Outermost radius | Minimum viewport width × height |
|---|---:|---:|
| 1 | 128 | 296 × 346 |
| 2 | 194 | 404 × 478 |
| 3 | 260 | 536 × 610 |
| 4 (daemon default) | 326 | 668 × 742 |
| 6 (engine limit) | 458 | 932 × 1006 |

These are logical sizes, not physical monitor sizes. At 2× a 3840×2160 display
offers 1920×1080 logical pixels; a 1920×1080 display offers only 960×540. The latter
falls back for four rings. Viewport bounds currently describe the selected monitor,
not an EWMH work area. Do not invent panel/work-area support in this milestone.

If `distance(anchor,C) > 0.5`, enter `ApproachingCenter` pointer state: hub and
approach guide are drawn, rings can be visible, but all pointer targets reject
until a **real motion sample** enters the hub (`distance(p,C) < h`). The anchor
disc (24 lp) and existing convex anchor-to-root corridor cover approach input.
Draw a subtle guide line underneath the rings ending at the hub. Ignore dwell
and band while approaching. Invoke keyboard navigation explicitly exits this
guard; returning to pointer requires entering the hub first. Never warp the
pointer toward C. This guard prevents a relocated annulus from becoming an
accidental target under the hand.

Fallback is a complete Branching session with existing placement, not a hybrid.
Record `InsufficientSpace` or `TargetTooSmall`; platform may select the same
fallback with `NoCompositor`. Preserve the preference for the next activation.
Scrolling and clipping are not overflow solutions in this release.

## I3. Separate logical path, hover and selection

Keep these concepts separate:

- `open_chain`: submenu `ItemPath`s, starting with `[]`; one per open ring.
- `on_path`: segment that opened the next ring; independent of pointer position.
- `raw_hit`: strict physical zone at the latest pointer position.
- `hovered_item` / keyboard focus: current explanation/highlight target.
- `pending_sibling`: a path and a monotonic adoption deadline, if switching an
  ancestor would replace an existing child.
- `eligible_pointer_target`: derived from strict hit, entry guard, transition
  guard, enabled ancestors, enabled item and current session phase.

Ring array indexes are presentation positions; `ItemPath` identifies a menu node.
An action ID is resolved only by the app's session `ActionIndex`. Retraction
snapshots do not join the logical path or participate in hit testing.

## I4. Hit testing and pointer eligibility

Test hub once, then radial intervals, then sectors. Do **not** call the existing
deepest-wheel-first `hit_test` on concentric geometries: every child would claim
inner ancestors as its hub. Return a dedicated `NestedHit`:
`Hub`, `Item { level,index }`, `RadialGap { inner_level }`,
`Separator { level }`, `OuterMargin`, `Approach`, or `Outside`.

Only the open annulus interiors are item zones. Exact inner/outer boundaries are
neutral. Between annuli the entire 6 lp gap is neutral at every angle. Capture
padding beyond the last ring is neutral. Caption and guide are neutral. Disabled
items still return an item for explanation but reject selection.

Angular hysteresis affects explanation/highlight only: retain the previous item
within `min(cfg.hysteresis_deg, Δ*0.15)` of its selectable span **within the same
ring**. Apply at most 3 lp radial hover retention into a gap, never across the
gap's midpoint. Strict commit geometry always wins: if retained hover differs
from strict hit, reject and show no armed highlight. Moving through a separator
must never commit the previous item. Do not change legacy Branching hysteresis.

When new root items, a new child, or a replacement child become available, record
`exposed_at_pointer`. A pointer target in that new geometry is armed only by a
subsequent motion at least 2 lp from that recorded point. Duplicate XDND positions,
ticks, opacity changes and enrichment do not count. The motion that opened a
child may not also select/open one of its children. At most one hierarchy change
per input sample. Keyboard activation bypasses this pointer-entry guard. Existing
6 lp activation slop from the original summon point applies as well.

Once armed, eligibility does not wait for animation completion. The full-size
ring is visible immediately and opacity animates on top of stable hit geometry.
See I8. On a leaf that becomes disabled, eligibility is revoked immediately;
re-enabling an unchanged sector may restore eligibility without pointer motion
because no geometry appeared or moved. Send unsolicited XDND status accordingly.

## I5. Hover, outward travel and changing branches

Timing sources are the existing validated engine config. Raw engine defaults are
140 ms dwell, 300 ms grace and 4° hysteresis. **Shipped app defaults are 110 ms,
180 ms and 3.5°**, mapped through `AppConfig::to_wheel_config`. Hub-close remains
150 ms, band fraction 0.65. Do not substitute values from older design examples.

| Situation | Required transition |
|---|---|
| Enabled submenu in deepest ring | Start dwell at first stable strict hit. Open at `entered_at + dwell_ms` if still that item; or open on an outward radial move of at least 2 lp with current radius at/above `inner + band*thickness`. |
| Opening child | Append next annulus; parent remains on path. Clear that dwell. Record new geometry's pointer-entry guard. No selection, no submenu-default execution. |
| Gap, separator, outer margin, outside | Clear selectable target and pending dwell; retain the entire open chain. Do not start a collapse timer. |
| Strict hit on existing open parent sector | Keep all descendants. Clear pending sibling. Parent may be a direct default-child target only under I7. |
| Strict hit on a different ancestor sector | Set pending sibling and `deadline = now + grace_ms`; retain old descendants, show candidate explanation, reject pointer commit until adopted. Reset deadline only when candidate changes, not on jitter or repeated samples. |
| Pending sibling stays under pointer at deadline | Close descendants after that level; adopt candidate. Enabled submenu opens immediately if dwell from candidate entry is already satisfied; otherwise wait until `max(entry+dwell, entry+grace)`. Leaf becomes selectable; disabled item remains explanation-only. |
| Deliberate outward push on pending sibling | If two successive strict samples are on that same sibling, radius increases ≥2 lp, and threshold is met, adopt/open immediately when band mode is enabled. First crossing into the sibling is never this shortcut. |
| Return to child/gap/open parent before deadline | Cancel pending sibling; keep old chain. |
| Inward move to any ancestor | Apply the same sibling rule; do not truncate just because radius decreased. |
| Hub | Reject immediately; after `hub_close_ms` continuous dwell truncate to root. Leaving earlier cancels hub timer. |

The **whole inter-ring gap plus pending-sibling grace** replaces the vine's intent
triangle. Concentric has no tangent triangle or child-center distance heuristic.
This permits crossing an ancestor sector diagonally to reach the existing child
without immediate collapse. A pending sibling never leaves the old parent as a
selectable stale target. Grace expiry re-evaluates the last strict hit and path
identity; it must not index a wheel that has since closed.

Click-only mode never opens by hover or push. Band-only never opens on dwell.
Zero durations expire at the same timestamp on the next processing step, without
an unbounded recursive open. `next_deadline` includes all active dwell, adoption,
hub and animation deadlines. Each timer clears itself before issuing a transition.

## I6. Invoke pointer and keyboard

Preserve `InvokeSelect::Click`: press and release on the same eligible leaf, or
press hub then release on an eligible leaf. `InvokeSelect::Release` remains
supported by the engine. Press/release identity includes path and geometry
generation, so a sibling replacement cannot turn a held press into a new action.
Clicking a submenu explicitly adopts/opens it, even during pending-sibling grace;
it does not execute its default child. Clicking disabled/separator/gap is a no-op
with no dispatch, matching the existing distinction between a non-action click
and session cancellation. Hub or outside click cancels; right-click retains app
cancellation behavior. Drag releases in all these non-action zones reject/end
the session, rather than staying open.

Keyboard: Next (`Right`, `Down`, `Tab`) and Prev (`Left`, `Up`, `Shift+Tab`) wrap
within the deepest ring, including disabled items. Activate (`Enter`, `Space`)
opens an enabled submenu and focuses its first item, or commits an enabled leaf.
Backspace closes exactly one ring and focuses its parent sector; at root it does
nothing. Digits 1–9 activate matching items; 10–12 are reached by cycling. Escape
cancels. Keyboard never uses pointer hover timers or auto-opens by dwell. Real
pointer movement ≥2 lp from the last keyboard takeover point switches back to
pointer interpretation, subject to the displaced-origin guard. Drag has no new
keyboard or scroll behavior and acquires no grabs.

## I7. Release, defaults, cancellation and menu updates

Concentric `drop_target()` is Accept only in Appearing/Open, after slop/entry
guards, with no pending sibling for that hit, with raw hit equal to the displayed
armed item, and with the item **and every ancestor** enabled. A leaf returns its
path. A directly hit submenu may return its immediate enabled leaf
`default_child`, preserving the existing model; do not recursively chase
defaults. All other zones reject, including a gap adjacent to a parent with a
default child. `commit_drop()` rechecks this synchronously; there is no latched
last-leaf acceptance. No geometry/timer event emits Commit.

The app then resolves the frozen action identity, verifies current contextual
enablement and dispatches through the existing pipeline. During provisional
payload completion match the captured action ID in the real menu; missing,
invisible or disabled action means failure, never “whatever now occupies this
path.” Reject sends `XdndFinished(false, None)` if a drop was delivered. Accept
uses Private. Never send MOVE, never request DELETE, never unmap on a layout
reload while the source still owns the drag.

Structure is immutable after `set_menu` installs the initial menu. Enrichment
does not reorder/remove sectors. Disabling an ancestor cancels affected timers
and eligibility but retains ring positions. Update caption reason immediately.
User edits to actions or layout are for the next session; background conversion
jobs are independent. See architecture for session snapshots and display changes.

## I8. Rendering, labels and animation

One central hub, no child hubs, no vine stems between rings. Use existing theme
colors and icons. Ancestors retain 0.72 opacity and on-path accent; active ring
has opacity 1.0. An opened parent has a filled rim marker; unopened submenus have
the existing hollow marker. Hover uses highlight plus a 2 lp **inside** outline,
with `pop = 0`; disabled sectors keep their layout with muted icon/text. A pending
sibling may show a low-intensity highlight (0.25), never the armed outline.

Place content at the sector midpoint, radius `inner + 0.55*thickness`, horizontal
orientation. Use 20 lp icons and 12 lp labels (root may use 22 lp icons). Starting
at width 128, compute the largest axis-aligned content box whose four corners and
edges remain inside that annular sector, with 4 lp padding. Use exact distance to
the box for inner-circle clearance; corners alone are insufficient. Height is
37 lp for icon+one-line label, 14 lp for label-only. If no width ≥48 fits, draw
only the centered 20 lp icon; if no icon exists, draw a short ellipsized label.
Store these boxes in geometry for both drawing and damage. Never shrink font
below 12 lp to fit. Cache text shaping; preserve Unicode strings and existing
missing-glyph fallback. Do not assume Control Center's system-font fallback also
exists in the wheel renderer.

The fixed 280×64 caption contains breadcrumb (11 lp, one line), hovered full
source label (13 lp, one line), and description/disabled reason (11 lp, up to two
lines). Shape/wrap then ellipsize to the box; arbitrary-length source text cannot
be guaranteed to fit. Retain complete strings in state/diagnostics. Breadcrumb
is a presentation of cached ancestor labels, not clickable navigation; elide
oldest middle ancestors first. Caption is never an action region. For a settled
path without hover, show its breadcrumb and “Move inward to go back.”

Nested expansion uses **opacity**, not geometric radius/scale animation. Root
and new ring begin at opacity 0.35, reach their target in 130 ms OutCubic. Full
geometry and labels are visible from the first frame; this avoids invisible
active sectors and prevents scaled annuli from covering ancestors. Retraction
fades current opacity to zero in 110 ms, immediately excluded from hit testing.
Reduced motion uses ≤60 ms opacity-only transitions. Hover in/out 90/120 ms;
ancestor dim 120 ms; caption 80 ms; reduced variants cap each at 60 ms.

Retarget tweens from their current value. On sibling replacement at a radial
slot, remove that slot's old ghost before drawing the new ring: never paint two
different action sets over one another. Keep at most one retracting snapshot per
level and at most six visible ring records total (active records take priority).
Cancel during expansion clears all target eligibility and finishes once. Keep
existing engine commit/dismiss phase semantics; the production controller hides
the overlay immediately on successful dispatch today, so do not delay actions
to expose a new commit animation. The sandbox can demonstrate the engine flash.

Scale is fixed per session. Physical input is `(root_position-overlay_origin)/s`;
multiply geometry by `s` once for rendering and shape conversion. Test 1, 1.25,
1.5, 2 and fractional rounding at boundaries. No per-ring or per-monitor scaling
within one session; X11 currently supplies global Xft.dpi, not independent monitor
DPI. New monitor/scale settings affect the next activation as specified in A7.
