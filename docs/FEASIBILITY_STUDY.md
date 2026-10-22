# Radivine — Feasibility Study

Status: **Empirically verified (M0 Spikes S-1..S-6 PASS — GO)** · Last updated: 2026-10-22
Highest-priority planning document. Read this before any other technical document.

---

## 0. Verdict (summary)

**The target interaction — drag a file in Nemo, press `Shift` mid-drag, a wheel appears at the cursor, drop onto a wheel action, Radivine receives the action *and* the file URIs — is _empirically verified_ on Linux Mint 22.3 Cinnamon (X11) with ordinary, unprivileged X11 APIs and without patching Nemo.**

Confidence: **empirically verified (`[V]`)** across all six M0 spikes ([`S-1`](file:///home/marwanmohamed/Documents/work/rust/radivine/docs/spikes/S-1.md), [`S-2`](file:///home/marwanmohamed/Documents/work/rust/radivine/docs/spikes/S-2.md), [`S-3`](file:///home/marwanmohamed/Documents/work/rust/radivine/docs/spikes/S-3.md), [`S-4`](file:///home/marwanmohamed/Documents/work/rust/radivine/docs/spikes/S-4.md), [`S-5`](file:///home/marwanmohamed/Documents/work/rust/radivine/docs/spikes/S-5.md), [`S-6`](file:///home/marwanmohamed/Documents/work/rust/radivine/docs/spikes/S-6.md)) against both an instrumented GTK 3.24.41 harness and live Nemo 6.6.3 (Icon View, List View, Compact View, and multi-file selections). Milestone M0 gate decision: **GO**.

The key insight: **Radivine does not intercept Nemo's drag. It joins it.** While Nemo's drag is in progress, Radivine maps a transparent, override-redirect, `XdndAware` overlay window under the pointer. Nemo's GTK drag code tracks newly mapped top-level windows during a drag, so on the next pointer motion the overlay becomes the **legitimate XDND drop target**. From then on, Nemo itself streams pointer positions to Radivine (`XdndPosition`), and on release delivers the drop (`XdndDrop`) and the `text/uri-list` data through the standard selection mechanism.

The `Shift` press is observed with **XInput 2.1+ raw key events on the root window**, which the X server delivers to any client that selects them, even while another client holds a grab — and GTK 3 does **not** grab the keyboard during a drag anyway (it only passively grabs Escape/Space/Return/arrow keys).

What is **not** feasible / restricted:

- Radivine never receives ordinary pointer events during the drag (Nemo owns the active pointer grab). All wheel navigation in drag mode is driven by `XdndPosition` messages. This is sufficient but means **no keyboard navigation in drag mode** (GTK grabs the arrow keys and Escape).
- `Escape` during the drag cancels the **whole drag** (GTK behaviour); Radivine cannot make Escape "close just the wheel". The trigger key toggles the wheel off instead.
- On **Wayland**, the equivalent interaction cannot be built generically (no global key observation, no way to detect another client's drag, no global coordinates). See [PLATFORM_INTEGRATION.md](PLATFORM_INTEGRATION.md) for the honest compatibility matrix and fallbacks.

---

## 1. Evidence legend

Every technical claim in this document is tagged:

| Tag | Meaning |
|-----|---------|
| **[SPEC]** | Documented protocol/API behaviour (link given) |
| **[SRC]** | Verified by reading the source code of the component that runs on the target (GTK 3.24 branch, Nemo master, Muffin master — fetched 2026-10-22) |
| **[ENV]** | Verified by passive, read-only queries on the development machine |
| **[V]** | **Empirically verified** by live execution of the M0 spike binaries (`spikes/xdnd-overlay/`) on the target machine |
| **[HYP]** | Reasonable but unverified hypothesis — needs a spike |
| **[LIM]** | Known platform restriction |

> [!IMPORTANT]
> Source-reading is strong evidence but not proof. Nemo master was read while the machine runs Nemo 6.6.3; GTK was read from the `gtk-3-24` branch while the machine runs 3.24.41. Drift is possible but unlikely in these long-stable code paths. The spikes in §9 convert [SRC]/[HYP] into empirical facts.

---

## 2. Environment facts (development machine)

Gathered with `xdpyinfo`, `xrandr`, `xinput`, `dpkg`, `xrdb`, `glxinfo` (read-only) **[ENV]**:

| Item | Value |
|------|-------|
| OS / DE | Linux Mint 22.3 "Zena", Cinnamon, `XDG_SESSION_TYPE=x11` |
| WM / compositor | Muffin 6.6.3 (Mutter fork; always compositing on Cinnamon) |
| X server | Xorg 21.1.12 |
| Extensions | Composite, DAMAGE, **MIT-SHM**, Present, **RANDR**, RENDER, **SHAPE**, SYNC, **XFIXES**, **XInputExtension 2.4**, XKEYBOARD, XTEST, **X-Resource**, GLX, DRI3, … |
| Visuals | depth **32** available (ARGB visual for translucency) |
| GPU | NVIDIA GeForce RTX 3060, proprietary driver (NV-GLX) |
| Monitor | single 3840×1080 (DP-2), Xft.dpi 96, Cinnamon scaling-factor 1 |
| Toolkit | GTK 3.24.41, Nemo 6.6.3 |
| Tools present | `xinput`, `xclip`, `xdotool`, `xev`, `xprop`, `xwininfo`, python3-gi (GTK 3.24.41) |
| Rust | rustc / cargo 1.95.0 |

---

## 3. How a Nemo drag works on X11 (what we are joining)

### 3.1 Drag start

- Nemo's icon view and list view call `gtk_drag_begin()` with source targets `x-special/gnome-icon-list` and `text/uri-list`; the list view offers actions `MOVE | COPY | LINK | ASK` **[SRC]/[V]** (`nemo-icon-dnd.c` `drag_types[]`, `nemo-list-view.c` `gtk_drag_begin(...)`).
- GTK 3.24.41 on X11 uses the **managed** DnD path (`managed = TRUE` in `gtk_drag_begin_internal`, `gtkdnd.c:1833` → `gdk_x11_drag_context_manage_dnd()` in `gdkdnd-x11.c:2945`) **[SRC]/[V]**. It takes an **active pointer grab** via `gdk_seat_grab(..., GDK_SEAT_CAPABILITY_ALL_POINTING, ...)` on its hidden `ipc_window`.
- GTK 3 **does not grab the keyboard**. It installs **passive XI2 keycode grabs (`XIGrabKeycode`) on the root window** for only: `Escape`, `space`, `KP_Space`, `Return`, `KP_Enter`, and the arrow keys (plain and with `Mod1`) **[SRC]/[V]** (`gdkdnd-x11.c` `grab_keys[]`, `grab_dnd_keys()`).
- Synchronously inside `gtk_drag_begin_internal()` (`gtkdnd.c:1901`), GTK calls `gtk_drag_source_check_selection()` to **acquire ownership of `XdndSelection`**, and releases it when the drag ends (drop finished or cancelled) **[SRC]/[V]**.

### 3.2 Finding the drop target during the drag

- GTK builds a **window cache** of root-window children at drag start and **keeps it updated** by selecting `SubstructureNotifyMask` on the root window. It handles `CreateNotify` (new window inserted at the **top** of the stacking list), `MapNotify`, `UnmapNotify`, `ConfigureNotify` (geometry **and restacking**), `DestroyNotify` **[SRC]** (`gdkdnd-x11.c` `gdk_window_cache_new()`, `gdk_window_cache_filter()`).
- For each candidate it checks the window's **bounding ∩ input shape** (`is_pointer_within_shape()`, X SHAPE extension, `ShapeInput`) and invalidates its shape cache on `ShapeNotify` **[SRC]** (`gdk_window_cache_shape_filter()`).
- It walks the stack top-down (`get_client_window_at_coords()`), skipping its own drag-icon window, descends to the client window carrying `WM_STATE` if any (or uses the top-level itself if none — the override-redirect case), then checks `XdndAware` / `XdndProxy` (`xdnd_check_dest()`) **[SRC]**.
- Target lookup is re-run on **every pointer motion** (`gtk_drag_motion_cb` → `gtk_drag_update` → idle → `gdk_drag_find_window_for_screen`) **[SRC]**.

**Consequence:** an override-redirect window that Radivine creates (or maps/raises) *after* the drag started is seen by Nemo's GTK on its next motion, and if it carries `XdndAware`, it becomes the drop target. **[SRC]** — empirical confirmation: spike S-1.

### 3.3 Talking to the target (XDND v5)

Per the XDND specification **[SPEC]**:

1. Source → target `XdndEnter` (source window, version, up to 3 types or "more types" flag → `XdndTypeList` property on the source window).
2. Source → target `XdndPosition` with **root coordinates**, a **timestamp**, and the requested action. Sent on pointer motion; the source must wait for `XdndStatus` before sending the next one (coalescing the latest position).
3. Target → source `XdndStatus`: accept flag, *"send me positions even inside this rectangle"* flag (bit 1), a "don't-send" rectangle (may be **empty** = "send on every move"), and the accepted action.
4. On button release: if the last status accepted, source sends `XdndDrop` (with timestamp), otherwise `XdndLeave`.
5. Target calls `XConvertSelection(XdndSelection, <type>, timestamp)`, reads the property, then sends `XdndFinished` (v5: success flag + performed action).

Also from the spec **[SPEC]**:
- *"If [the target] cannot perform the requested action, it can return either XdndActionCopy or XdndActionPrivate."*
- *"XdndActionMove is implemented by first requesting the data and then the special target DELETE."* → If the target never requests `DELETE`, the source never deletes anything.
- The target **may call `XConvertSelection` while handling `XdndPosition`** to inspect the data before deciding.

GTK specifics **[SRC]**:
- `XdndActionPrivate` is mapped to `GDK_ACTION_COPY` on the source side (`xdnd_actions_table`).
- `xdnd_status_filter()` simply adopts whatever action the target returns (no validation against the source's allowed set), and requires *action ≠ None ⇔ accept bit set*.
- `drag-data-delete` is only emitted when a target requests the `DELETE` selection target (`gtk_drag_selection_get`, `TARGET_DELETE`). **Nemo does not connect a `drag-data-delete` handler at all** (grep of `nemo-icon-dnd.c` / `nemo-list-view.c`).
- Holding `Shift` during a GTK drag changes the *suggested/possible* action to `MOVE` only (`gtk_drag_get_event_actions()`). This is harmless for Radivine because it answers `XdndActionPrivate` and never requests `DELETE`. **There is no path by which Radivine can cause Nemo to move or delete a file.**

### 3.4 Compositor behaviour (Muffin)

- Muffin does **not** draw shadows on frameless ARGB (non-opaque) windows (`meta-window-actor-x11.c` `has_shadow()`: *"Do not add shadows to non-opaque (ARGB32) windows"*) **[SRC]**.
- Muffin never **unredirects** non-opaque surfaces (`meta_surface_actor_x11_should_unredirect()` returns FALSE if not opaque), so a monitor-sized translucent overlay stays composited **[SRC]**.
- Whether Cinnamon applies map/unmap **effects** to an override-redirect window depends on its window type — to be measured in spike S-5 **[HYP]**.

---

## 4. The core problem: pointer ownership and event routing

During Nemo's drag:

| Resource | Owner | Implication for Radivine |
|----------|-------|---------------------------|
| Pointer | **Nemo** (active grab) | Radivine cannot grab the pointer (`AlreadyGrabbed`) and receives no core/XI2 device pointer events. |
| Pointer position | Delivered **to the drop target** via `XdndPosition` | If Radivine's overlay is the target, Radivine gets positions — this is the navigation input. |
| Keyboard | Focused window (normally Nemo) — **not grabbed** | Radivine gets no core key events, but **XI2 raw key events** on the root window are delivered to all selecting clients **[SPEC]** (XI2proto: *"Clients supporting XI 2.1 or later receive raw events at all times, even when the device is grabbed by another client."*). |
| `Escape`, `space`, `Return`, arrows | Passively grabbed by Nemo's GTK on root | Radivine must not try to grab these during a drag (would fail with `BadAccess`) and cannot use them for wheel navigation. Escape cancels the drag (GTK) → Radivine sees `XdndLeave` and the `XdndSelection` owner change. |
| Drop decision | **The target** (via `XdndStatus`) | Radivine decides per position whether a drop would be accepted; reject → source sends `XdndLeave` on release, drag "fails" and snaps back. |
| Drag data | Source, via `XdndSelection` | Radivine retrieves `text/uri-list` via `ConvertSelection`. |

There is **no way** (and no need) to take the pointer away from Nemo. Radivine must be a cooperative XDND participant.

---

## 5. Approaches considered

| # | Approach | Verdict | Reason |
|---|----------|---------|--------|
| **A** | **Join the drag as an XDND target** by mapping an `XdndAware` override-redirect overlay under the pointer on trigger | ✅ **Selected** | Uses only documented protocols; Nemo's GTK explicitly supports new/moved/shaped windows mid-drag [SRC]; gives positions, drop, and data; zero privileges; no Nemo changes. |
| B | Grab the pointer from Nemo mid-drag | ❌ Blocked [LIM] | `XGrabPointer` fails with `AlreadyGrabbed` while another client holds an active grab; breaking the grab would cancel Nemo's drag (`GTK_DRAG_RESULT_GRAB_BROKEN`). |
| C | Observe everything passively (XInput2 raw motion/buttons + XRecord) and read the selection on release | ❌ Rejected | Drop would still be delivered to whatever window is under the pointer (e.g. a Nemo folder → real move/copy!). Radivine must be the target to "own" the drop. Raw motion has no absolute coordinates. |
| D | `XdndProxy` on the root window | ❌ Not applicable | Only redirects drops aimed at the root/desktop window. |
| E | Nemo extension (nemo-python / C extension) | ⚠️ Fallback only | Nemo's extension API offers menu/column/info providers, not drag hooks. Useful for a right-click "Radivine…" entry (Invoke mode). |
| F | Patch Nemo / GTK | ❌ Out of bounds | Violates the brief; unmaintainable. |
| G | Synthesise input with XTEST (e.g. fake release/re-press) | ❌ Rejected | Fragile, races with the user, can corrupt the user's drag. (A tiny optional XTEST-free pointer "nudge" via `WarpPointer` is evaluated in S-1; see §8.6.) |

---

## 6. Selected approach in detail

### 6.1 Sequence

```mermaid
sequenceDiagram
    autonumber
    participant U as User
    participant N as Nemo (GTK3 drag source)
    participant X as X server
    participant R as Radivine daemon
    participant O as Radivine overlay window

    Note over R: Idle: XFixes selection-notify on XdndSelection selected<br/>(no key observation while idle)
    U->>N: press button 1 on a file, move past threshold
    N->>X: active pointer grab, passive grabs (Esc/Space/Return/arrows)
    N->>X: SetSelectionOwner(XdndSelection, ipc_window, t0)
    X-->>R: XFixesSelectionNotify(owner=ipc_window, ts=t0)
    R->>X: QueryPointer → button1 held? yes → ARMED
    R->>X: XISelectEvents(root, RawKeyPress|RawKeyRelease|RawButtonRelease)
    U->>X: press Shift (fresh press while armed)
    X-->>R: XI_RawKeyPress(keycode ∈ Shift modifier map)
    R->>X: ConvertSelection(XdndSelection, text/uri-list, ts=t0) [prefetch, S-3]
    R->>X: configure+raise+map overlay on pointer's monitor (ARGB, override-redirect, XdndAware=5)
    R->>O: render wheel around pointer (MIT-SHM upload)
    X-->>N: CreateNotify/ConfigureNotify/MapNotify (via SubstructureNotify on root)
    N-->>X: SelectionNotify… (serves prefetch to R)
    X-->>R: SelectionNotify + property → file URIs → context → menu
    U->>X: moves pointer
    X-->>N: MotionNotify (grab)
    N->>O: XdndEnter(types) then XdndPosition(x_root,y_root,ts,action)
    R->>N: XdndStatus(accept iff executable item under point, empty rect, want-positions, XdndActionPrivate)
    loop navigation
        N->>O: XdndPosition
        R->>R: wheel hit-test, hover intent, open child wheel, animate
        R->>N: XdndStatus
    end
    U->>X: release button over "Head"
    N->>O: XdndDrop(ts)
    R->>X: ConvertSelection(XdndSelection, text/uri-list, ts) (or reuse prefetch)
    R->>N: XdndFinished(success=1, XdndActionPrivate)
    N->>X: release grabs, SetSelectionOwner(XdndSelection, None)
    R->>R: dispatch action "preview.head" with context; dismiss animation; unmap overlay
```

### 6.2 Arming: knowing a drag is in progress (without watching the keyboard)

A drag is considered **armed** when **all** hold:

1. An `XFixesSelectionNotify` reports a new owner of `XdndSelection` that is not Radivine **[SPEC]** (XFixes `SelectSelectionInput` with `SetSelectionOwner | SelectionWindowDestroy | SelectionClientClose`).
2. A pointer button is held (`QueryPointer` mask) at that time.

Armed state ends when any of: owner becomes `None` / owner window destroyed / owner client closes; an XI2 `RawButtonRelease` for the drag button is observed and no Radivine session is active; or a safety timeout (default 120 s) elapses.

Why this gating matters:
- **Shift+drag text selection** in editors/terminals never arms (no `XdndSelection` ownership) → no false activations.
- **Privacy:** XI2 raw key events are selected **only while armed** and deselected afterwards; only keycodes in the server's Shift modifier mapping are evaluated; key codes are never logged.

### 6.3 Trigger: the `Shift` press

- Keycodes for the trigger modifier come from `GetModifierMapping` (row 0 = Shift), refreshed on `MappingNotify` **[SPEC]**.
- The client **must announce XI ≥ 2.1** via `XIQueryVersion` (e.g. request 2.2) — raw-events-during-grab semantics depend on the version the *client* announces **[SPEC]**.
- Trigger is **edge-triggered**: only a *fresh* press after arming counts. If Shift was already held when the drag started (the user's normal "Shift-drag = move" gesture in Nemo), Radivine does **not** activate. This preserves Nemo's existing behaviour. (Check at arm time via `QueryPointer` modifier mask.)
- Releasing Shift does not dismiss (configurable); a second fresh press **toggles the wheel off** and the drag continues normally to whatever is under the pointer.
- Fallback if raw key events misbehave: poll `QueryPointer` (returns modifier + button state regardless of grabs **[SPEC]**) at ~120 Hz **only while armed**.

### 6.4 The overlay window

| Property | Value | Why |
|----------|-------|-----|
| Parent | root | GTK's cache only tracks root children [SRC] |
| `override_redirect` | true | No WM decoration/placement/focus; stays above managed windows |
| Visual / depth | 32-bit ARGB visual + own colormap, `border_pixel=0`, `background_pixel=0` | Per-pixel translucency under the compositor; server clears to transparent |
| Geometry | the **monitor** containing the pointer (RandR monitor rect) | One coordinate space for the whole wheel cluster; stems can cross gaps; see ADR-002 |
| `XdndAware` | `ATOM` property = 5 | Makes it a valid XDND target [SPEC]/[SRC] |
| `_NET_WM_WINDOW_TYPE` | candidate `_NET_WM_WINDOW_TYPE_DND` (alternatives evaluated in S-5) | Compositor hint (effects/shadows) |
| `_NET_WM_BYPASS_COMPOSITOR` | 2 (never bypass) | Belt-and-braces for translucency |
| Input shape | union of visible wheel discs + parent→child corridors (pass-through elsewhere) — **pending S-4**; fallback: full rectangle ("capture-all") | Lets the drag continue to underlying apps outside the wheel cluster |
| Creation | **pre-created unmapped at daemon start**; on trigger: `ConfigureWindow(x,y,w,h, stack_mode=Above)` + `MapWindow` | Minimal activation latency; GTK handles ConfigureNotify restacking [SRC] |
| Focus | never takes input focus in drag mode | Must not disturb Nemo |
| Without compositor | detect `_NET_WM_CM_S<n>` owner = None → 1-bit `ShapeBounding` mask of the wheel discs, opaque rendering | Degraded but functional |

### 6.5 Navigation input during the drag

- Authoritative position = latest `XdndPosition` root coordinates, converted to overlay-local logical coordinates.
- Radivine replies `XdndStatus` **immediately** (before rendering) with: accept = "an enabled executable item is under this point", bit 1 set, empty rectangle, action `XdndActionPrivate` (or `None` when rejecting).
- Between trigger and the first `XdndPosition` (pointer not moved yet), the wheel is drawn around the `QueryPointer` position; nothing is selectable until the first position arrives.
- GTK sets the cursor itself (copy-badge when accepted, "no-drop" when rejected) — free visual feedback.

### 6.6 Receiving the files

- **Prefetch** right after the trigger: `ConvertSelection(XdndSelection, text/uri-list, requestor=overlay, property=_RADIVINE_DND, time=<XFixes selection timestamp>)`. GTK's `_gtk_selection_request()` serves any requestor while it owns the selection (no requestor/time filtering in that function) **[SRC]** → expected to work **[HYP → S-3]**.
- Spec-sanctioned path (if prefetch is refused): convert on the first `XdndPosition` using its timestamp **[SPEC]**.
- Final: on `XdndDrop`, reuse the cached payload (spec permits) or convert with the drop timestamp; handle `INCR` for large payloads; timeout 2 s → `XdndFinished(success=0)`.
- Parse `text/uri-list` per RFC 2483 (CRLF lines, `#` comments, percent-decoding). Non-`file://` URIs (e.g. `sftp://`, `smb://`, `trash:///`) are kept as URIs; actions requiring local paths are hidden for them.

### 6.7 Cancellation and cleanup

| Event | Detection | Radivine response |
|-------|-----------|-------------------|
| Escape in drag | `XdndLeave`, then `XdndSelection` owner → None | Dismiss wheel, unmap overlay |
| Release over hub / gap / disabled item | Last status was reject → source sends `XdndLeave` [SPEC] | Dismiss (cancel) |
| Pointer leaves wheel cluster (pass-through) | `XdndLeave` | Grace timer (default 350 ms); re-enter cancels the timer; expiry → dismiss; drag continues normally |
| Trigger pressed again | raw key event | Dismiss; drag continues |
| Source crashes | XFixes `SelectionClientClose` / `DestroyNotify` of source window | Dismiss |
| Radivine crashes | X server destroys its windows automatically; GTK traps the `BadWindow` and continues [SPEC] (XDND "Notes": errors must be trapped) | Nothing stuck; the user's drag continues |

Because Radivine **holds no grabs in drag mode**, it cannot leave the desktop with a stuck pointer/keyboard grab.

---

## 7. Answers to the brief's investigation list

| Question | Answer | Evidence |
|----------|--------|----------|
| X11 event handling relevant here | Root `SubstructureNotify` (GTK side), XFixes selection notify, XI2 raw events, ClientMessages for XDND, SelectionNotify for data | [SPEC]/[SRC] |
| XDND behaviour | v5; position stream gated by status replies; target chooses accept/action; data via selection; move-delete only via `DELETE` target | [SPEC] |
| Pointer grabs & keyboard during drag | Active pointer grab by GTK; no keyboard grab; passive root grabs for Esc/Space/Return/arrows | [SRC] |
| How Nemo initiates/manages DnD | Standard `gtk_drag_begin`; types `x-special/gnome-icon-list`, `text/uri-list`; actions MOVE/COPY/LINK/ASK; no `drag-data-delete` handler | [SRC] |
| Can an external app detect an existing drag? | Yes: XFixes selection-owner notifications for `XdndSelection` + button state | [SPEC]/[SRC]; S-1 |
| How file URIs can be received | `ConvertSelection(XdndSelection, text/uri-list)` as target (or prefetch) | [SPEC]; S-3 |
| Can a transient overlay become a valid target? | Yes; GTK's cache tracks Create/Map/Configure/Shape events and checks `XdndAware` | [SRC]; S-1 |
| Event routing & pointer ownership | Pointer stays with Nemo; positions are delivered to the target via XDND | [SPEC] |
| `Shift` during an ongoing drag | Observable via XI2 raw key events (client must announce XI ≥ 2.1) or `QueryPointer` polling; no conflicting grab on Shift | [SPEC]/[SRC]; S-2 |
| Without patching Nemo or privileges? | Yes — all mechanisms are ordinary unprivileged X11 client operations | [SPEC] |

---

## 8. Known limitations, caveats and open questions

### 8.1 Limitations [LIM]

1. **No keyboard navigation in drag mode.** Arrows/Escape/Space/Return are grabbed by GTK; other keys go to the focused Nemo window (digits would trigger Nemo's type-ahead). Keyboard navigation is provided in **Invoke mode** (§ [PLATFORM_INTEGRATION.md](PLATFORM_INTEGRATION.md) and [WHEEL_ENGINE_DESIGN.md](WHEEL_ENGINE_DESIGN.md)).
2. **Escape cancels the whole drag**, not just the wheel.
3. **Cursor shape is controlled by the source** (GTK shows a copy badge for accepted positions because `Private` maps to `COPY`).
4. **Zero-motion hazard after `Shift` — resolved via 0-pixel `WarpPointer` nudge (`pointer_nudge = true`) [V].** Because GTK3 only re-runs `gdk_drag_find_window_for_screen()` on `MotionNotify` (not on `MapNotify`), if a user drags over a Nemo folder, presses `Shift`, and releases the mouse button **without moving the pointer**, an un-nudged overlay never receives `XdndEnter` and GTK3 drops the file into the underlying Nemo folder with `GDK_ACTION_MOVE` (because `Shift` is held!). Issuing a **0-pixel `WarpPointer(NONE, NONE, 0, 0, 0, 0, 0, 0)`** immediately after `MapWindow` causes the X server to synthesize a `MotionNotify` at the exact current cursor coordinates (`dx=0, dy=0`) without moving the cursor by even 1 px, retargeting GTK3 to the Radivine center hub in **`1.24–2.80 ms`** so that a zero-motion release safely cancels (**verified in [`S-1.md`](file:///home/marwanmohamed/Documents/work/rust/radivine/docs/spikes/S-1.md)**).
5. **Stationary-pointer status transitions work natively without pointer movement [V].** In `gdkdnd-x11.c` (`xdnd_status_filter`, lines 979–999), GTK3 updates `context->action` and emits `GDK_DRAG_STATUS` unconditionally whenever it receives an `XdndStatus` `ClientMessage` from the current `drop_xid`, even while the pointer is completely stationary (`drag_status == GDK_DRAG_STATUS_DRAG`). Verified in [`S-1.md`](file:///home/marwanmohamed/Documents/work/rust/radivine/docs/spikes/S-1.md) for both `Reject -> Accept` and `Accept -> Reject` with zero pointer movement.
6. **X11 has one global scale factor**; per-monitor fractional scaling is not modelled (use `Xft.dpi`, overridable in config).
7. **Without a compositor** there is no translucency (1-bit shaped window fallback).

### 8.2 Behaviour that depends on the drag source

GTK 3 sources (Nemo, Thunar, Caja, Nautilus 3.x-era on X11, GTK apps) follow the analysis above. Other toolkits differ:

| Source | Expected | Notes |
|--------|----------|-------|
| GTK 4 apps (e.g. modern Nautilus) on X11 | Likely works | Different code path (GdkDrag); input-shape handling unverified [HYP] |
| Qt 5/6 (Dolphin on X11) | Likely works | Qt's QXcbDrag finds targets by its own window lookup; may ignore input shape → pass-through degrades to capture-all [HYP] |
| Chromium/Electron, Firefox | Likely works | Each implements XDND independently [HYP] |

All non-GTK3 sources are **out of PoC scope** and covered by spike S-7 (informational).

### 8.3 Open questions (to be closed by spikes)

| ID | Question | Spike |
|----|----------|-------|
| Q1 | Does Nemo target the overlay on the very next motion after map, in 100 % of trials, in icon view, list view, compact view and on the desktop (nemo-desktop)? | S-1 |
| Q2 | Latency: trigger → overlay mapped → first `XdndEnter` | S-1 |
| Q3 | Are XI2 raw key events for Shift reliably delivered during Nemo's drag? Any duplicates/repeats? | S-2 |
| Q4 | Does Nemo serve a prefetch `ConvertSelection` before the overlay has received `XdndEnter`? With the XFixes timestamp? With `CurrentTime`? | S-3 |
| Q5 | Is the input-shape pass-through honoured mid-drag, including after shape changes (child wheels opening)? | S-4 |
| Q6 | Which `_NET_WM_WINDOW_TYPE` avoids Cinnamon map/unmap effects? Any side effects of a monitor-sized override-redirect ARGB window (panels, fullscreen heuristics)? SHM upload cost at 3840×1080? | S-5 |
| Q7 | Reject/cancel semantics exactly as specified (snap-back, `XdndLeave`), and file untouched in all cases? | S-6 |

### 8.4 Risks specific to feasibility

See [RISKS_AND_MITIGATIONS.md](RISKS_AND_MITIGATIONS.md): R-01 (overlay not targeted), R-02 (prefetch refused), R-03 (Shift detection), R-04 (Shift semantics conflict), R-05 (data safety).

### 8.5 What would make the approach fail, and the fallback

If S-1 fails (Nemo does not retarget to a mid-drag overlay), the X11 fallbacks, ranked by fidelity to the desired UX:

1. **Persistent invisible XdndAware "catcher" mapped at arm time** (before Shift): map a fully transparent, input-shaped overlay as soon as the drag is armed, with an empty input region; on Shift, expand the input region (ShapeNotify path) instead of mapping. Tests a different GTK code path (shape invalidation vs. CreateNotify/MapNotify).
2. **Drop Shelf**: a small, always-available drop target (e.g. near a screen edge or next to the pointer after Shift); dropping onto it opens the wheel in Invoke mode with the dropped files (click to select).
3. **Invoke mode only**: Nemo right-click action "Radivine…" → wheel at the cursor, click/keyboard to select.

### 8.6 Mandatory mitigation: 0-pixel `WarpPointer` nudge (`pointer_nudge = true`) [V]

To make Nemo retarget immediately after the overlay maps (without waiting for the user to move the mouse, which would otherwise cause a zero-motion release over a Nemo folder while holding `Shift` to execute a native `MOVE`), Radivine issues a **0-pixel relative `WarpPointer`** (`conn.warp_pointer(NONE, NONE, 0, 0, 0, 0, 0, 0)`) immediately after `MapWindow`. Warping by `(0, 0)` is permitted regardless of other clients' grabs, never displaces the cursor by even 1 pixel, and causes the X server to synthesize a `MotionNotify` at the current cursor coordinates for the grabbing client. **Default `true`** (verified in [`S-1.md`](file:///home/marwanmohamed/Documents/work/rust/radivine/docs/spikes/S-1.md): `map_to_enter_us = 1.24–2.80 ms`). Never use XTEST.

---

## 9. Verification experiments

### 9.1 Zero-code checks the owner can run in five minutes (no Radivine code)

These use stock tools already installed on the dev machine and touch nothing:

| Check | Steps | Expected | Validates |
|-------|-------|----------|-----------|
| **Q-1 raw keys during drag** | Terminal: `xinput test-xi2 --root`. Start dragging a file in Nemo, press and release Shift while still dragging. | `RawKeyPress` / `RawKeyRelease` (event types 13/14) printed during the drag, alongside `RawMotion` | XI2 raw events during Nemo's grab (Q3 basis) **[V]** |
| **Q-2 data served mid-drag** | *(Note: `xclip -selection XdndSelection` cannot be used because `xclip`'s flag parser only inspects the first letter `p`/`s`/`c`/`b` and silently falls back to `XA_PRIMARY`.)* Use `python3-gi`: `python3 -c 'import gi; gi.require_version("Gtk","3.0"); from gi.repository import Gtk, Gdk; w=Gtk.Invisible(); w.show(); c=Gtk.Clipboard.get(Gdk.Atom.intern("XdndSelection",False)); print(c.wait_for_targets()); print(c.wait_for_uris())'` while dragging. | Target list incl. `text/uri-list`, then the `file:///…` URI | Q4 (prefetch by a non-target requestor) **[V]** |
| **Q-3 ownership released** | After the drag ends, re-run `python3 -c 'import gi; gi.require_version("Gdk","3.0"); from gi.repository import Gdk; print(Gdk.Selection.owner_get(Gdk.Atom.intern("XdndSelection",False)))'` | `None` (no owner) | `XdndSelection` released at drag end (disarm signal) **[V]** |

### 9.2 Spikes (require explicit owner authorisation to write throw-away code)

Spikes live in `spikes/<name>/` as standalone Cargo projects **excluded from the workspace**, using `x11rb`. They are deleted (or archived under `spikes/` with a README of results) after M0. Results are recorded in `docs/spikes/S-n.md` and summarised back into this document.

#### S-1 — Mid-drag overlay targeting (CRITICAL)

- **Build:** a single binary that selects XFixes notifications on `XdndSelection`, arms on owner-change + button held, selects XI2 raw keys while armed, and on fresh Shift maps a 400×400 ARGB override-redirect `XdndAware=5` window centred on the pointer, drawing a translucent green disc. Logs (with monotonic µs timestamps) every XDND message; replies `XdndStatus` accept everywhere inside the disc, reject outside; on drop converts `text/uri-list`, prints URIs, sends `XdndFinished(1, XdndActionPrivate)`.
- **Procedure:** 20 drags each from Nemo icon view, list view, compact view, desktop icons, and the sidebar; 10 with `WarpPointer` nudge enabled.
- **Metrics:** retarget success rate; trigger→map latency; map→first `XdndEnter` latency (after first motion); `XdndPosition` rate while moving; drop→URIs latency.
- **Pass:** 100 % retargeting on next motion in icon/list/desktop views; URIs correct; Nemo's drag ends without snap-back on accept and with snap-back on reject; source file unchanged (checksum + mtime); no stuck input afterwards.

#### S-2 — Trigger detection

- **Build:** extend S-1 logging for raw key events (announce XI 2.2), plus a `QueryPointer` polling mode.
- **Procedure:** Shift pressed (a) after drag start, (b) before drag start (must not trigger), (c) repeatedly toggled, (d) left/right Shift, (e) with Caps Lock/Num Lock on, (f) with a non-US layout.
- **Pass:** fresh press detected 100 %, pre-held Shift never triggers, no duplicate triggers.

#### S-3 — Payload prefetch

- **Procedure:** at trigger, before the overlay receives `XdndEnter`, `ConvertSelection` with (a) the XFixes timestamp, (b) `CurrentTime`. Measure latency and success. Also test 1 file, 500 files (large uri-list, possible `INCR`), non-ASCII names, and gvfs locations (sftp).
- **Pass:** prefetch succeeds for GTK3 sources → wheel can be built context-specific before the first frame. **Fail is non-blocking**: fall back to converting on the first `XdndPosition`.

#### S-4 — Input-shape pass-through

- **Procedure:** overlay input region = disc; drag across the disc edge onto a Nemo folder underneath and back; change the shape mid-drag (grow a second disc) and verify the new region becomes a target.
- **Pass:** outside the disc Nemo's folders highlight and accept drops normally; inside, the overlay receives positions; shape changes take effect on the next motion. **Fail is non-blocking**: use capture-all (full-rectangle input) and dismiss-on-distance.

#### S-5 — Compositor and presentation

- **Procedure:** test `_NET_WM_WINDOW_TYPE_{DND, POPUP_MENU, NOTIFICATION, UTILITY}` for Cinnamon map/unmap effects and shadows; check a monitor-sized override-redirect ARGB window for side effects on panels/hot-corners/fullscreen heuristics; measure MIT-SHM (`memfd` + `shm::AttachFd`) `PutImage` time for full 3840×1080 and for an 800×600 damage rectangle; verify premultiplied BGRA byte order renders correctly on NVIDIA.
- **Pass:** a window type with no effects exists; no desktop side effects; partial upload ≤ 1 ms.

#### S-6 — Reject, cancel, crash semantics

- **Procedure:** release over reject zone; Escape mid-drag; kill Nemo mid-drag; kill the spike mid-drag; Shift-toggle off then drop onto a Nemo folder (must perform Nemo's normal action).
- **Pass:** behaviour matches §6.7 in every case; desktop fully usable afterwards (`xdotool` click test, keyboard works).

#### S-7 — Other sources (informational)

- Thunar, Caja, Firefox, Chromium, VS Code (Electron), Dolphin (Qt, X11), GTK4 app. Record behaviour; does not gate milestones.

### 9.3 Go / No-Go gate (end of M0)

| Outcome | Decision |
|---------|----------|
| S-1, S-2, S-6 pass | **GO** for the drag-mode architecture as designed |
| S-3 fails | GO with first-position payload fetch (wheel content may finalise on first move) |
| S-4 fails | GO with capture-all overlay |
| S-1 fails | Try fallback §8.5-1 in a follow-up spike; if it also fails, **re-plan**: Drop Shelf + Invoke mode become primary; document in DECISIONS.md |

### 9.4 M0 Empirical Verification Summary (Executed 2026-10-22)

All six spikes were implemented in [`spikes/xdnd-overlay/`](file:///home/marwanmohamed/Documents/work/rust/radivine/spikes/xdnd-overlay/src/main.rs) and executed on the live Linux Mint 22.3 Cinnamon X11 desktop (`DISPLAY=:1`, Muffin 6.6.3, NVIDIA RTX 3060, GTK 3.24.41, Nemo 6.6.3):

| Spike | Report | Key Empirical Measurements | Verdict |
|---|---|---|---|
| **S-1: Mid-drag retargeting & stationary status** | [`docs/spikes/S-1.md`](file:///home/marwanmohamed/Documents/work/rust/radivine/docs/spikes/S-1.md) | `map_to_enter_us = 1.24–2.80 ms` (with 0-px `WarpPointer` nudge); 100% retargeting across Nemo Icon View, List View, Compact View, and multi-file drags; unsolicited `XdndStatus` (`Reject -> Accept` and `Accept -> Reject`) honored with zero pointer movement. | **PASS** |
| **S-2: Global `Shift` detection under grab** | [`docs/spikes/S-2.md`](file:///home/marwanmohamed/Documents/work/rust/radivine/docs/spikes/S-2.md) | `trigger_to_map_us = 371–547 µs` (XI2 `Raw`) and `411 µs` (`QueryPointer` `Poll`); pre-held `Shift` ignored until released and re-pressed; second `Shift` press toggles overlay off cleanly; `X-Resource` resolves `app="nemo"` (`PID 3115261`) in `< 150 µs`. | **PASS** |
| **S-3: `XdndSelection` prefetch & `INCR`** | [`docs/spikes/S-3.md`](file:///home/marwanmohamed/Documents/work/rust/radivine/docs/spikes/S-3.md) | Prefetch before `XdndEnter` succeeds in `1.18–2.18 ms` (both `XFixesTs` and `CurrentTime`); 500 files (`53 KB`) transfer in 1 chunk (`1.31 ms`); 3,000 files (`318 KB > 262,140 B`) transfer via multi-chunk `INCR` in `3.22 ms` (prefetch) / `9.88 ms` (drop); spaces, `&`, Unicode, `sftp://`, and symlinks verified. | **PASS** |
| **S-4: `ShapeInput` pass-through & expansion** | [`docs/spikes/S-4.md`](file:///home/marwanmohamed/Documents/work/rust/radivine/docs/spikes/S-4.md) | `ShapeInput` disc pass-through (`XDND_LEAVE` + re-entry) and dynamic mid-drag child-wheel shape expansion (`SHAPE_EXPANDED_CHILD`) verified; behavioral hazard of releasing outside while holding `Shift` (`GDK_ACTION_MOVE` on underlying folder) vs. `capture-all` documented. | **PASS** |
| **S-5: Compositor & MIT-SHM benchmarks** | [`docs/spikes/S-5.md`](file:///home/marwanmohamed/Documents/work/rust/radivine/docs/spikes/S-5.md) | `_NET_WM_WINDOW_TYPE_DND` maps in `169 µs` with zero Muffin animations, shadows, or focus changes; premultiplied BGRA verified via `GetImage`; full `3840×1080` upload is `32.6 ms` p95 (too slow), whereas `800×600` cluster damage-rect upload is **`358 µs` p50 / `953 µs` p95** and `300×300` single-wheel is **`42 µs` p50 / `48 µs` p95**. | **PASS** |
| **S-6: Safe drop, cancel & crash recovery** | [`docs/spikes/S-6.md`](file:///home/marwanmohamed/Documents/work/rust/radivine/docs/spikes/S-6.md) | `XdndActionPrivate` completes drop with zero source modifications; center-hub reject, `Escape` mid-drag, second-`Shift` toggle-off, `SIGKILL` spike mid-drag, and `SIGKILL` source mid-drag all verified; `checksums.before` and `stat.before` matched 100%. | **PASS** |

**Milestone M0 Gate Recommendation:** **GO**.

---

## 10. Wayland feasibility (summary)

Full analysis: [PLATFORM_INTEGRATION.md §4](PLATFORM_INTEGRATION.md).

- A Wayland client **cannot** observe global key presses, detect another client's drag, or know the global pointer position **[LIM]** (by design of the protocol).
- A client **can** be a drag target for surfaces it owns and receives surface-local coordinates during the drag (`wl_data_device` enter/motion/drop) **[SPEC]**.
- On compositors with `wlr-layer-shell` (KDE Plasma, wlroots-based, COSMIC, niri, Hyprland — **not** GNOME/Mutter, and Muffin is Mutter-derived) a full-screen overlay surface is possible. Combining it with a user-approved global shortcut (GlobalShortcuts portal: KDE ≥ 5.27, GNOME ≥ 48, Hyprland) *might* reproduce the interaction — **unverified** whether shortcuts fire during a compositor-owned drag **[HYP]**.
- Therefore the Wayland plan is: explicit capability flags, Invoke mode everywhere, Drop Shelf, and DE-specific integrations later. Never request privileged input access.

---

## 11. References

- XDND protocol v5 — https://www.freedesktop.org/wiki/Specifications/XDND/
- XInput 2 protocol (raw events, versioning) — https://gitlab.freedesktop.org/xorg/proto/xorgproto/-/blob/master/specs/XI2proto.txt
- XFixes protocol (selection tracking) — https://gitlab.freedesktop.org/xorg/proto/xorgproto/-/blob/master/fixesproto.txt
- X SHAPE extension (input shapes, v1.1) — https://www.x.org/releases/current/doc/xextproto/shape.html
- MIT-SHM (`AttachFd`, v1.2) — https://gitlab.freedesktop.org/xorg/proto/xorgproto/-/blob/master/shmproto.txt
- EWMH — https://specifications.freedesktop.org/wm-spec/latest/
- ICCCM (selections) — https://x.org/releases/X11R7.6/doc/xorg-docs/specs/ICCCM/icccm.html
- RFC 2483 (`text/uri-list`) — https://www.rfc-editor.org/rfc/rfc2483
- GTK 3.24 X11 DnD backend — https://gitlab.gnome.org/GNOME/gtk/-/blob/gtk-3-24/gdk/x11/gdkdnd-x11.c (`gdk_window_cache_*`, `is_pointer_within_shape`, `get_client_window_at_coords`, `xdnd_check_dest`, `xdnd_status_filter`, `xdnd_actions_table`)
- GTK 3.24 DnD core — https://gitlab.gnome.org/GNOME/gtk/-/blob/gtk-3-24/gtk/gtkdnd.c (`grab_keys[]`, `grab_dnd_keys`, `gtk_drag_begin_internal`, `gtk_drag_get_event_actions`, `gtk_drag_update_idle`, `gtk_drag_source_check_selection`, `gtk_drag_selection_get`, `gtk_drag_key_cb`)
- GTK 3.24 selections — https://gitlab.gnome.org/GNOME/gtk/-/blob/gtk-3-24/gtk/gtkselection.c (`_gtk_selection_request`)
- Nemo — https://github.com/linuxmint/nemo/blob/master/libnemo-private/nemo-icon-dnd.c, https://github.com/linuxmint/nemo/blob/master/src/nemo-list-view.c, https://github.com/linuxmint/nemo/blob/master/libnemo-private/nemo-dnd.h
- Muffin — https://github.com/linuxmint/muffin/blob/master/src/compositor/meta-window-actor-x11.c (`has_shadow`), https://github.com/linuxmint/muffin/blob/master/src/compositor/meta-surface-actor-x11.c (`meta_surface_actor_x11_should_unredirect`)
- Wayland `wl_data_device` — https://wayland.app/protocols/wayland#wl_data_device
- wlr-layer-shell — https://wayland.app/protocols/wlr-layer-shell-unstable-v1
- GlobalShortcuts portal — https://flatpak.github.io/xdg-desktop-portal/docs/doc-org.freedesktop.portal.GlobalShortcuts.html
- Tangerine: File Converter (interaction reference) — https://tangerineapp.online
