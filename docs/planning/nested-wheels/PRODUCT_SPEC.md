# Optional Concentric / Nested Wheels — product specification

Status: **Proposed; planning only; owner review required before implementation.**
Investigated baseline: `e6a342e`, released v0.2.0, 2026-10-25. Target: **v0.3.0**.

This package does not authorize an implementation milestone. The repository's
`AGENTS.md` milestone gate remains binding. Its M6-era project-state paragraph is
historical; the inspected C1–C5 commits and the owner's release statement establish
v0.2.0 as the baseline. Do not modify that release or its artifacts.

## P1. Product contract

Add a Wheel Layout preference with exactly two choices:

| Choice | Behavior |
|---|---|
| **Branching / Vine** | Existing adjacent circles, bridge sectors, vine stems, geometry, hover rules and animations. Default for missing settings, new installations and existing installations. |
| **Concentric / Nested** | A root wheel and successive full circular annuli sharing one fixed center. Each visible annulus represents one submenu on the current open path. |

Both layouts consume the same finalized `MenuTree` and `ActionIndex`. They expose
the same visible actions, order, icons, contextual enablement, disabled reasons,
automatic More… groups, and depth-limit behavior. The layout does not register
actions, resolve conversion presets, start jobs, or modify action arguments.
One open child per level remains the rule. All open ancestors remain visible.

The root is centered on the activation pointer when the reserved cluster fits.
Near an edge, clamp the **whole reserved cluster once before mapping**. That
clamped center is the interaction origin for every ring. There is no cursor warp,
recenter, rotation, resizing, ring scrolling, or replacement of ancestors during
navigation. A displaced root has a visible origin-to-hub guide and a neutral
approach corridor; arriving at a segment while approaching the hub cannot select
an action. See [interaction contract](INTERACTION_DESIGN.md#i2-placement-and-overflow).

## P2. Normal interaction

1. Nemo Drag + fresh Shift or Invoke opens the selected layout. No-motion release
   rejects, including when the root was displaced.
2. Hovering an enabled submenu opens its next annulus after the configured dwell,
   or an outward push opens it sooner. Existing click-only/band-only engine modes
   remain supported; the daemon currently uses dwell plus band.
3. Move through the neutral inter-ring gap, then around the child ring to any
   option. The entire circumference belongs to that submenu. Ancestor sectors on
   the open path retain their path accent even while another ring is hovered.
4. Moving inward preserves descendants while crossing gaps or the open parent
   sector. Deliberately dwelling on an ancestor sibling changes the path. Hovering
   the central hub for `hub_close_ms` closes descendants back to the root.
5. Drag release on an eligible enabled leaf dispatches its existing action.
   Invoke click and keyboard activation use the same action pipeline. Hovering,
   opening, closing, and animation completion never execute an action.

Detailed rules, boundary ownership, deadlines and release eligibility are fixed
in [INTERACTION_DESIGN.md](INTERACTION_DESIGN.md); they are not left to Gemini.
Concentric has stricter neutral-gap release rules than Branching. This is an
intentional property of the new layout, not a change to legacy hit testing.

## P3. Space, density and fallback

Use full-circle rings. Restricting a child to its parent's angular slice would
multiply density at each depth; full rings maintain predictable targets for
different item counts. Depth is communicated by separate annuli, parent-sector
accents, a marker at each opened parent's outer rim, and a caption breadcrumb.

Reserve space for `WheelConfig.max_depth` **before the session starts**, including
for a Drag placeholder whose menu is not available yet. Production currently
uses four levels and eight items per level; engine hard limits remain six and
twelve. Child rings are 60 logical pixels thick, with 6 logical pixels between
rings. Root radii use the existing validated root settings.

If the reservation or minimum target checks fail, use Branching for the **entire
session**, preserving the requested preference. Do not drop actions, reduce the
configured menu depth, squeeze targets, clip selectable sectors, or silently
switch layout halfway through a path. Show “Using Branching: nested rings need
more space” in the fallback caption when no item is hovered; item explanations
take priority. Record requested/effective layout and reason in status diagnostics.
The default four-ring reservation fits a 1366×768 display at scale 1; a 1280×720
display uses Branching with the shipped root radii. High scale can also cause
fallback. See exact arithmetic in I2.

This conservative reservation can select Branching for a shallow actual menu on
a small screen. That is the chosen tradeoff for stable origins and identical
placeholder/Invoke behavior. Paging, compressed ancestor breadcrumbs in place of
rings, and wheel scrolling are deferred to a separately authorized design.

For this release, lack of a compositing manager also selects Branching before
mapping. The existing opaque fallback is retained; this feature does not claim
to repair its existing input-bounding limitations. Platform limitations are
explicit in [architecture A7](ARCHITECTURE.md#a7-x11-safety-and-platform-boundaries).

## P4. Configuration and Control Center

The only new persisted key is:

```toml
version = 1

[wheel]
layout = "branching" # valid values: "branching", "concentric"
```

Values are exact lowercase strings. Missing means `branching`. An unknown string
or wrong TOML value type for this key produces a warning and falls back **only
this field** to Branching; it must not reset trigger, safety, jobs, or other
settings. A syntactically invalid TOML file retains the existing whole-file
startup fallback. Strict save validation rejects an invalid layout instead of
silently saving it. No schema-version bump or migration of actions/convert files.

Put the selector first on the existing Wheel page, with these descriptions:

- Branching: “Submenus open beside their parent, connected by vine stems.”
- Concentric: “Submenus open as rings around one center. Uses Branching when the
  rings cannot fit on this display.”
- Shared help: “Applies to the next wheel activation. An open wheel stays as it is.”

Reuse Save Changes, Restore Defaults, conflict detection, atomic persistence and
background `ReloadDaemon`. Restore Defaults restores Branching. Show existing
root radii and timings for both layouts. Group child radii and child offset gap
under “Branching geometry”; disable those controls when Concentric is selected,
preserving their values. Display “Nested ring thickness: 60 px; gap: 6 px” as
read-only explanatory text. Do not add new geometry sliders in this release.
Existing root values that cannot supply usable nested targets cause session
fallback, not destructive rewriting of the user's settings.

An optional static egui schematic may illustrate the two choices; it is not a
second geometry implementation or a requirement for the first release. Do not
generate or modify protected branding/icon assets.

Saving is persistent immediately. The daemon receives the existing Reload IPC;
effective layout changes on the **next activation**. Opening, active, left-grace,
pending-drop and dismissing sessions retain their configuration and registry
snapshot. Repeated saves replace the next-session defaults; they never dismiss,
remap, release input capture, or convert a rejected drop into acceptance. If IPC
fails, report “Saved; daemon reload failed” and retain the saved file, rather than
claiming the running daemon adopted it. Jobs already running continue unchanged.

## P5. Cancellation and availability

`input.passthrough = false` remains the safe default. While Radivine is the XDND
target, releasing on the hub, gaps, separator lines, disabled items, a closed or
retracting ring, a protected transition, or outside the active ring cluster
rejects. No action is dispatched. Never request MOVE or DELETE. A second fresh
Shift press intentionally returns control to the native drag, as today.

With explicitly enabled pass-through, or after moving to another monitor outside
the single overlay, Nemo may own the drop. Native drops there are not Radivine
cancellations. The selector must neither enable pass-through nor expand this
existing limitation. Test transitions within the overlay against real folders
using disposable fixtures and compare source location as well as checksums.

Visibility and ordering are frozen for the displayed menu. Enrichment updates
enablement/reasons in place. Disabled items keep their sector and remain focusable
for explanation. A disabled ancestor makes descendants ineligible immediately;
the engine may keep their geometry visible until normal navigation closes it.
Action and menu reloads apply to the next session. A provisional drag selection
is bound to its action ID before the real payload arrives; never reuse its old
numeric path against a newly built menu.

## P6. Release and review

Recommend v0.3.0: this is a new optional interaction mode and a backward-compatible
config addition, larger than a v0.2.x maintenance change. Keep v0.2.0 tags, notes,
packages and release assets intact. Develop using the isolated environment in
`docs/DEVELOPMENT.md`; no installed service or Nemo changes are authorized by this
planning task.

The proposed defaults are complete. Owner review should accept or reject the
startup fallback tradeoff, the amount of edge displacement, and the visual/UX
result at N5. There is no unresolved engine algorithm for Gemini to choose.
Any demand for guaranteed concentric operation on smaller displays requires a
separate product decision expanding scope to overflow navigation. The proposed
ADR remains Proposed through implementation until owner acceptance.
