# Radivine — Platform Integration

Status: **Proposed (X11 normative for M2–M3; others are extension plans)** · Last updated: 2026-10-22
Prerequisite reading: [FEASIBILITY_STUDY.md](FEASIBILITY_STUDY.md).

---

## 1. Strategy

1. **X11 first, done properly.** The X11 backend implements the full drag-mode interaction by joining the source's XDND session (FEASIBILITY_STUDY §6). It is the reference behaviour.
2. **Capabilities, not pretence.** Every backend reports a `Capabilities` struct. The app adapts its UX to what is supported and tells the user honestly (`radivine doctor`, first-run notification) — it never fakes the drag interaction.
3. **Portable core, per-OS edges.** The wheel engine, renderer, context and actions are platform-independent. A backend only has to: report events, show a transparent always-on-top surface, present premultiplied RGBA pixels, act as a drop target, and (optionally) grab input for Invoke mode.
4. **No security bypasses.** No `/dev/input`, `uinput`, `input` group, root helpers, accessibility-API abuse, or disabling of compositor protections — on any platform.

## 2. Capability model

```rust
pub enum Support { Yes, Degraded(&'static str), No(&'static str) }

pub struct Capabilities {
    pub drag_session: Support,       // trigger mid-drag + overlay becomes the drop target
    pub invoke_at_pointer: Support,  // Invoke mode wheel positioned at the pointer
    pub translucency: Support,       // per-pixel alpha overlay
    pub input_passthrough: Support,  // overlay input region limited to wheel shapes
    pub keyboard_in_drag: Support,   // keyboard navigation during drag mode
    pub clipboard_files: Support,    // own the clipboard with file lists (Copy action)
}
```

### 2.1 Compatibility matrix (honest, as of 2026-10)

| Capability | X11 + compositor (Cinnamon/Muffin) | X11 without compositor | Wayland + layer-shell (KDE, wlroots, COSMIC, niri, Hyprland) | Wayland GNOME (Mutter) | Wayland Cinnamon (Muffin) | Windows 10/11 | macOS |
|---|---|---|---|---|---|---|---|
| Drag-mode wheel (trigger mid-drag) | **Yes (Verified M0 S-1/S-2)** | Yes | **No** generic path; *hypothesis* W-A (portal shortcut + overlay) | **No** (extension needed) | **No** (extension needed) | Likely (spike) | Likely (spike; Tangerine proves feasibility) |
| Invoke-mode wheel at pointer | Yes | Yes | Yes (full-screen layer surface learns pointer on enter) | Degraded (centred window) | Degraded (centred window) | Likely | Likely |
| Translucent overlay | Yes | Degraded (1-bit shaped, opaque) | Yes | Yes (normal window) | Yes (normal window) | Yes (layered window) | Yes (NSPanel) |
| Input pass-through outside wheels | **Yes (Verified M0 S-4)** | Yes | Yes (`wl_surface.set_input_region`) | n/a | n/a | Partial | Partial |
| Keyboard nav in drag mode | **No** (source grabs keys) | No | n/a | n/a | n/a | Unknown | Unknown |
| Clipboard file lists (Copy) | Yes | Yes | Yes (`wl_data_device` selection while focused) / data-control | Yes (focused) | Yes (focused) | Yes | Yes |
| Drop Shelf fallback | Yes | Yes | Yes | Yes (normal window) | Yes (normal window) | Yes | Yes |

"Likely" = supported by documented APIs, not yet spiked. Only the X11 column is in scope for v0.1.

## 3. X11 backend (normative for M2–M3)

### 3.1 Module layout (`radivine-platform/src/x11/`)

| Module | Responsibility |
|--------|---------------|
| `mod.rs` | `X11Platform` — implements `Platform`; owns `RustConnection`, screen info, sub-components |
| `atoms.rs` | `x11rb::atom_manager!` with every atom below |
| `overlay.rs` | Visual/colormap selection, window creation, properties, show/hide/configure, Expose handling |
| `shm.rs` | `memfd` SHM segment, `AttachFd`, swizzled copy of damaged rows, `shm::PutImage` with completion gating; core `PutImage` fallback |
| `shape.rs` | Input region → 1-bit pixmap → `shape::Mask(SET, INPUT)`; bounding mask for the no-compositor fallback |
| `arming.rs` | XFixes `XdndSelection` tracking, button state, XI2 raw key/button selection while armed, modifier map, trigger edge logic |
| `xdnd.rs` | **Pure** XDND target state machine (parsed messages in → replies/events out); unit-tested without an X server |
| `selection.rs` | `ConvertSelection`, property reading, `INCR` receive, timeouts; CLIPBOARD owner (serve targets, `INCR` send) |
| `invoke.rs` | Pointer/keyboard grabs with retry, event translation, guaranteed ungrab |
| `monitors.rs` | RandR 1.5 monitors + refresh rates, `Xft.dpi` (RESOURCE_MANAGER), XSETTINGS `Net/EnableAnimations`, change notifications |
| `source.rs` | X-Resource `QueryClientIds(LocalClientPID)` → `/proc/<pid>/comm` for `SourceInfo` |
| `doctor.rs` | Capability probe used by `radivine doctor` |

x11rb features: `shm`, `shape`, `xfixes`, `xinput`, `randr`, `res`, `resource_manager`, `cursor`.

### 3.2 Connection and extensions

At startup (fail = capability downgrade, never a crash):

| Extension | Version requested | Used for | If missing |
|-----------|------------------|----------|------------|
| XFixes | 5.0 | Selection-owner notifications | `drag_session = No("XFixes missing")` |
| XInputExtension | **2.2** (must be ≥ 2.1 for raw-events-during-grab) | Raw key/button events | Poll `QueryPointer` while armed |
| SHAPE | 1.1 | Input region, no-compositor bounding shape | `input_passthrough = No`; capture-all |
| MIT-SHM | 1.2 (`AttachFd`) | Fast presentation | Core `PutImage` (chunked to max request length) |
| RANDR | 1.5 | Monitors, refresh, change events | Single monitor = root geometry |
| X-Resource | 1.2 | Source PID | `SourceInfo.app = None` |

Compositor: `GetSelectionOwner(_NET_WM_CM_S<screen>) != None` → translucency; track changes with XFixes on that selection (compositor restarts).

### 3.3 Atoms

`XdndAware, XdndEnter, XdndPosition, XdndStatus, XdndLeave, XdndDrop, XdndFinished, XdndSelection, XdndTypeList, XdndActionCopy, XdndActionMove, XdndActionPrivate, text/uri-list, UTF8_STRING, TEXT, TARGETS, MULTIPLE, INCR, CLIPBOARD, x-special/gnome-copied-files, text/plain;charset=utf-8, _RADIVINE_DND, _RADIVINE_CLIP, _NET_WM_WINDOW_TYPE, _NET_WM_WINDOW_TYPE_DND, _NET_WM_WINDOW_TYPE_POPUP_MENU, _NET_WM_WINDOW_TYPE_NOTIFICATION, _NET_WM_WINDOW_TYPE_UTILITY, _NET_WM_BYPASS_COMPOSITOR, _NET_WM_NAME, _NET_WM_PID, WM_CLASS, RESOURCE_MANAGER, _XSETTINGS_SETTINGS` + per-screen `_NET_WM_CM_S0`, `_XSETTINGS_S0`.

### 3.4 Overlay window

Created once at startup, unmapped:

1. **Visual**: first `TrueColor` visual of depth 32 with masks `R=0x00FF0000 G=0x0000FF00 B=0x000000FF`. Create a colormap for it.
2. **Window**: child of root; geometry = primary monitor (reconfigured per session); attributes: `background_pixel=0`, `border_pixel=0`, `colormap`, `override_redirect=1`, `event_mask = Exposure | StructureNotify | PropertyChange`.
3. **Properties**: `XdndAware = [5]` (ATOM), `_NET_WM_WINDOW_TYPE = [_NET_WM_WINDOW_TYPE_DND]` (verified in Spike [`S-5`](file:///home/marwanmohamed/Documents/work/rust/radivine/docs/spikes/S-5.md): maps in `169 µs` with zero Muffin animations, shadows, or focus changes), `_NET_WM_BYPASS_COMPOSITOR = 2`, `WM_CLASS = "radivine\0Radivine\0"`, `_NET_WM_NAME = "Radivine"`, `_NET_WM_PID`.
4. A second **hidden InputOnly 1×1 window** (never mapped) is used as the XFixes selection-input window and as CLIPBOARD owner/requestor for non-session work.

Showing is split in two calls so the app can compute the wheel layout (which needs the monitor geometry and scale) and set the input region **before** the window is mapped:

`prepare_overlay(at) -> OverlayInfo`:
1. Pick the RandR monitor containing `at` (fallback: primary).
2. If geometry differs: `ConfigureWindow(x, y, w, h)`; (re)allocate the SHM buffer if the size changed.
3. Return `OverlayInfo { origin, size, scale, monitor, translucent }`. The window stays unmapped.

The app then creates the `WheelSession`, converts `input_region()` to `ShapePx` and calls `set_input_region(Some(..))` when `input.passthrough = true` (this always includes the anchor disc, so the drag source can retarget immediately even when the root wheel is displaced from the pointer) or `set_input_region(None)` when `input.passthrough = false` (capture-all mode).

> [!IMPORTANT]
> **Behavioral Trade-Off (`input.passthrough`, verified in Spike [`S-4`](file:///home/marwanmohamed/Documents/work/rust/radivine/docs/spikes/S-4.md)):**
> - **`input.passthrough = true` (Shaped Cluster):** Outside the wheel discs + corridors, the pointer passes through to the underlying application (`XDND_LEAVE` with `drag_ended = false`), and dynamic mid-drag shape expansion (`ShapeMask(SO::SET, SK::INPUT)`) immediately adds newly opened child wheels. However, if the user moves the pointer outside the wheel over a Nemo folder **while still holding `Shift`** and releases the mouse button expecting "clicking/releasing outside a menu cancels the menu", GTK3 executes a **native `MOVE`** into that underlying folder!
> - **`input.passthrough = false` (Capture-All):** The overlay's `ShapeInput` remains unclipped (`None`), so moving anywhere outside the wheel replies `XdndStatus(accept=false, None)` and releasing the mouse button outside the wheel **always safely cancels the drag**. To resume a normal Nemo drag, the user presses `Shift` a second time (toggle-off).

`map_overlay()`:
1. `ConfigureWindow(stack_mode = Above)`, `MapWindow`.
2. When `pointer_nudge = true` (**default `true`**, verified in Spike [`S-1`](file:///home/marwanmohamed/Documents/work/rust/radivine/docs/spikes/S-1.md)), immediately issue a **0-pixel relative `WarpPointer`** (`conn.warp_pointer(NONE, NONE, 0, 0, 0, 0, 0, 0)`) and flush. This causes the X server to synthesize a `MotionNotify` at the current cursor position (`dx=0, dy=0`) without moving the cursor by even 1 px, so GTK3 retargets to the Radivine overlay in `1.24–2.80 ms` (preventing a zero-motion release over a Nemo folder while holding `Shift` from moving the file).
3. `present()` calls made before the first `Expose` are coalesced and executed on `Expose` (drawing into an unviewable redirected window is lost).

`hide_overlay()`: clear input region to empty, `UnmapWindow`, and if a drag is still armed (`pointer_nudge = true`), issue a 0-pixel `WarpPointer(NONE, NONE, 0, 0, 0, 0, 0, 0)`, then flush. `release_buffers()` detaches and frees SHM (called by the app `overlay.release_after_s` seconds after the last session, default 30 s).

### 3.5 Presentation (MIT-SHM)

- Buffer: `memfd_create("radivine-shm")`, `ftruncate(w·h·4)`, `mmap(MAP_SHARED)`; `shm::attach_fd(seg, fd, read_only=true)`. memfd avoids leaking SysV segments if the process dies.
- Pixel format: renderer output is **premultiplied RGBA8**; the depth-32 ZPixmap on a `LSBFirst` server with the masks above is **B,G,R,A bytes** (`[60, 76, 4, 235]` for `#044C3C` @ 92% alpha, verified via `GetImage` in Spike [`S-5`](file:///home/marwanmohamed/Documents/work/rust/radivine/docs/spikes/S-5.md)) → swap R/B while copying **only damaged rows/columns** into the SHM buffer. Derive the shuffle from `setup.image_byte_order` + visual masks (do not hard-code).
- `shm::put_image(..., src rect = damage rect, send_event = true)`; do not write into the SHM buffer again until the matching `ShmCompletion` arrives — if a new frame is ready earlier, coalesce damage and present on completion.
- Fallback: core `PutImage` of damaged rects, split to respect `maximum_request_length` (BIG-REQUESTS).
- **Measured cost (Spike [`S-5`](file:///home/marwanmohamed/Documents/work/rust/radivine/docs/spikes/S-5.md) on NVIDIA RTX 3060 @ 3840×1080):**
  - `300×300` single wheel (`0.36 MB`): swizzle `58 µs` p50 / `63 µs` p95; `PutImage+Completion` **`42 µs` p50 / `48 µs` p95** (total `101 µs` p50 / `110 µs` p95).
  - `800×600` cluster (`1.92 MB`): swizzle `319 µs` p50 / `342 µs` p95; `PutImage+Completion` **`358 µs` p50 / `953 µs` p95** (total `682 µs` p50 / `1,281 µs` p95).
  - `3840×1080` full screen (`16.59 MB`): `PutImage+Completion` is `2,834 µs` p50 / **`32,597 µs` p95** — **never upload the full-screen buffer; always clip `shm::PutImage` to the frame's damage rectangles.**

### 3.6 Drag arming and trigger detection (privacy-minimised)

**Idle**: only `xfixes::select_selection_input(hidden_win, XdndSelection, SET_SELECTION_OWNER | SELECTION_WINDOW_DESTROY | SELECTION_CLIENT_CLOSE)`. No key or pointer observation.

**On `XFixesSelectionNotify`**:
- `owner ≠ None` and owner is not ours → `QueryPointer`; if any button is down: **Armed**. Record `selection_timestamp`, the button mask, and whether the trigger modifier is already held (`held_at_arm`). Select XI2 on root for `XIAllMasterDevices`: `RawKeyPress | RawKeyRelease | RawButtonRelease`. Emit `PlatformEvent::DragArmed { source }`.
- `owner = None` / window destroyed / client closed → **Disarmed**: deselect XI2 raw events; emit `DragDisarmed`.

**Trigger keycodes**: from `GetModifierMapping` (Shift row by default; Control/Mod1/Mod4 rows for other configured triggers). Refresh on `MappingNotify`.

**Edge logic** (in `arming.rs`, unit-tested):
- Track the set of trigger keycodes currently down (dedupe repeats).
- If `held_at_arm`, ignore presses until all trigger keys have been released once.
- A transition from "none down" to "one down" while Armed → `PlatformEvent::Trigger { at: QueryPointer }`.
- Release → `TriggerReleased` (the app decides; default ignore).

**Disarm watchdogs**: raw button release of the armed button while **no** session is active → disarm after 200 ms if the selection owner did not change (some sources keep owning `XdndSelection` after a drag). Absolute timeout 120 s.

**Fallback** (`[platform.x11] trigger_detection = "poll"`): while Armed, a 8 ms calloop timer polls `QueryPointer` for the modifier mask and button mask. Used automatically if XI < 2.1.

Privacy guarantees (also in user docs): raw key events are selected **only while a drag is armed**; only keycodes mapped to the trigger modifier are inspected; key data is never logged or stored.

### 3.7 XDND target protocol (exact messages)

All client messages are format 32; `l[n]` = `data32[n]`. Replies are sent with `SendEvent(propagate=false, destination=source_window, event_mask=0)`.

| Incoming | Fields | Backend action |
|----------|--------|----------------|
| `XdndEnter` | `l0` source window; `l1` bit0 = more than 3 types, bits 24–31 = version; `l2..l4` types | Record source + version (use `min(version, 5)`); types from `l2..l4` or read `XdndTypeList` (ATOM[]) from the source window; emit `DndEnter { types }` |
| `XdndPosition` | `l0` source; `l2` = `(x_root << 16) | y_root`; `l3` timestamp; `l4` requested action | Store timestamp; emit `DndPosition { at }`. The app **must** call `dnd_reply()` synchronously while handling it |
| `XdndLeave` | `l0` source | `QueryPointer`: `drag_ended = no button held ∨ XdndSelection owner gone`; emit `DndLeave { drag_ended }`; forget source |
| `XdndDrop` | `l0` source; `l2` timestamp | Store drop timestamp; emit `DndDrop`; the app calls `request_payload()` (or uses cache) then `dnd_finish()` |

| Outgoing | Fields |
|----------|--------|
| `XdndStatus` (from `dnd_reply(accept)`) | `l0` = overlay window; `l1` = `(accept ? 1 : 0) | 2` (bit 1: send positions inside rect); `l2` = 0, `l3` = 0 (empty rectangle); `l4` = `accept ? XdndActionPrivate : None` |
| `XdndFinished` (from `dnd_finish(success)`) | `l0` = overlay window; `l1` = `success ? 1 : 0`; `l2` = `success ? XdndActionPrivate : None` (v5) |

Rules:
- **Never** answer with `XdndActionMove`; **never** convert the `DELETE` target.
- Messages whose `l0` does not match the current source are ignored (stale).
- If the source window is destroyed mid-session (`BadWindow` on send, or `DestroyNotify` if selected), emit `DndLeave { drag_ended: true }` + `DragDisarmed`.
- `dnd_reply()` may also be called **unsolicited** (without a pending `XdndPosition`) when the app's drop target changes due to timers or enablement updates; **verified in Spike [`S-1`](file:///home/marwanmohamed/Documents/work/rust/radivine/docs/spikes/S-1.md)** that GTK 3.24 (`gdkdnd-x11.c` `xdnd_status_filter`) honors unsolicited `XdndStatus` messages immediately (`Reject → Accept` and `Accept → Reject`) with zero pointer movement. The drop-time re-check by the app remains as defense-in-depth.
- `xdnd.rs` exposes a pure `XdndTarget` struct: `fn on_message(&mut self, msg: ClientMessage) -> Option<DndEvent>`, `fn status(&self, accept) -> [u32; 5]`, `fn finished(&self, success) -> [u32; 5]`.

### 3.8 Payload retrieval

`request_payload(mime)`:
- Timestamp: drop timestamp if dropping, else last `XdndPosition` timestamp, else the XFixes `selection_timestamp` (prefetch before Enter; S-3), else `CurrentTime`.
- `ConvertSelection(requestor=overlay, selection=XdndSelection, target=<mime atom>, property=_RADIVINE_DND, time)`.
- On `SelectionNotify` with `property = None` → `PayloadError::Refused`. Otherwise `GetProperty(delete=true, long_length=max)`; if type is `INCR`, follow the INCR protocol (wait for `PropertyNotify(NewValue)`, read chunks, until zero-length).
- Timeout 2 s (`PayloadError::Timeout`). Size cap 16 MiB (`PayloadError::TooLarge`).
- Emit `PlatformEvent::Payload { mime, result }`.

### 3.9 Invoke mode (no drag in progress)

- `prepare_overlay` + `set_input_region(None)` (= **whole overlay**: clicks outside wheels must reach Radivine to cancel) + `map_overlay`.
- `grab_input()`: `GrabPointer(owner_events=false, mask=ButtonPress|ButtonRelease|PointerMotion, grab_window=overlay, cursor=left_ptr from the cursor theme)` and `GrabKeyboard(owner_events=false)`. On `AlreadyGrabbed` (e.g. Nemo's context menu still closing) retry every 10 ms for up to 300 ms. If the keyboard grab still fails, continue pointer-only (Esc unavailable; click outside cancels); if the pointer grab fails, continue without grab (overlay is on top anyway).
- Events → `PointerMoved`, `Button`, `Key` (mapped to `NavKey` by the app: arrows, Tab, Enter, Space, Backspace, Esc, digits).
- `ungrab_input()` on every exit path; additionally the X server releases grabs automatically when the overlay is unmapped or the client disconnects.
- Watchdog: Invoke sessions auto-cancel after 60 s without input.

### 3.10 Monitors, scale, settings

- RandR `GetMonitors` (+ refresh from CRTC modes); select `RRScreenChangeNotify` → `MonitorsChanged` (the app dismisses an active session).
- Scale = `Xft.dpi / 96` from `RESOURCE_MANAGER` (Cinnamon sets 192 at 2×), overridable by `appearance.scale`; `PropertyNotify` on root → `SettingsChanged`.
- Reduced motion: XSETTINGS (`_XSETTINGS_S0` owner's `_XSETTINGS_SETTINGS`, setting `Net/EnableAnimations`) — optional, M6.

### 3.11 No-compositor fallback (M6)

If no compositing manager: use a 24-bit visual, opaque rendering with the theme background, and a `ShapeBounding` 1-bit mask equal to the wheel shapes (hard edges, no translucency, no shadows). Capability `translucency = Degraded`.

### 3.12 Clipboard service (Copy action, M4)

- Own `CLIPBOARD` with the hidden window. Serve `TARGETS`, `x-special/gnome-copied-files` (`"copy\n" + uris joined by "\n"`), `text/uri-list` (CRLF), `UTF8_STRING` and `text/plain;charset=utf-8` (local paths joined by `"\n"`). Support `INCR` for large lists and `MULTIPLE`.
- On `SelectionClear`, drop the data. Ownership persists while the daemon runs (it is long-lived, so pasting later works).
- Requested by the app on the UI thread in response to an action effect (`Effect::SetClipboard`), never by a worker thread.

### 3.13 Error policy

X errors are expected (sources die, windows vanish). The backend checks replies with `.check()` where the result matters; otherwise errors arrive as events, are logged at `debug`, and translated into `DndLeave`/`DragDisarmed` when they concern the current source. A lost connection emits `ConnectionLost` → the app exits cleanly (autostart/systemd may restart it).

## 4. Wayland strategy

### 4.1 Restrictions (by design of Wayland) [LIM]

- No client can observe global keyboard input or another client's pointer.
- No client can learn the global pointer position or place a normal toplevel at arbitrary coordinates.
- No client can detect that another client started a drag. The compositor routes `wl_data_device` events only to the surface under the pointer.
- `wlr-layer-shell` gives panels/overlays but **does not** grant global input. GNOME (Mutter) does not implement layer-shell; Cinnamon's Wayland compositor (Muffin, Mutter-derived) is assumed not to either (verify when relevant).

### 4.2 What Wayland does allow

- A client **is** a normal drag target for its own surfaces and receives surface-local coordinates during the drag (`wl_data_device.enter/motion/leave/drop`, `wl_data_offer.accept/set_actions/finish`).
- A full-screen transparent layer-shell surface on the `overlay` layer with a controllable input region (`wl_surface.set_input_region`).
- User-consented global shortcuts via the **GlobalShortcuts portal** (`org.freedesktop.portal.GlobalShortcuts`; KDE Plasma ≥ 5.27, GNOME ≥ 48, Hyprland). Bare modifiers like a lone `Shift` are generally not bindable.

### 4.3 Options ranked by fidelity to the target UX

| Rank | Option | Description | Status |
|------|--------|-------------|--------|
| W-A | **Portal shortcut during drag + layer-shell overlay** | User binds e.g. `Super+Space` via the portal. During a drag they press it; Radivine maps a full-screen overlay layer surface; on the next motion the compositor sends `wl_data_device.enter` with coordinates → wheel at the pointer; drop is delivered normally. | **Hypothesis** — unverified whether compositors deliver global shortcuts during a compositor-owned drag, and whether a newly mapped surface receives `enter` mid-drag. Spike **W-A** when Wayland work starts. Layer-shell compositors only. |
| W-B | **Drop Shelf** | A small persistent drop target (layer-shell edge bubble, or a normal small window on GNOME). Dragging onto it opens the wheel anchored at the shelf (drag continues into the expanded overlay where supported; otherwise switch to Invoke-style click selection with the dropped files). | Feasible with documented APIs |
| W-C | **File-manager integration → Invoke mode** | Right-click → "Radivine…" (Nemo action, Nautilus script, Dolphin service menu, Thunar custom action) runs `radivine invoke %U`; wheel opens at the pointer (layer-shell: pointer learned on `wl_pointer.enter` of a full-screen surface) or centred (GNOME/Cinnamon). | Feasible |
| W-D | **DE-native integrations** | GNOME Shell extension, KWin script/effect, Cinnamon (CJS) extension running inside the compositor — these *can* see drags and keys legitimately within their DE. | Future, per-DE effort |
| W-E | XWayland | Only for X11 clients dragging between X11 windows; not a product path. | Informational |

### 4.4 Graceful fallback UX

- On startup in a Wayland session, `capabilities.drag_session = No(...)`; the daemon shows a one-time notification: *"Drag-and-Shift isn't available on Wayland. Use right-click → Radivine…, or the drop shelf."*
- `radivine doctor` prints the full capability matrix for the running session.
- Features never silently degrade into something that looks like drag mode but isn't.

### 4.5 Implementation outline (future)

`radivine-platform::wayland` with `smithay-client-toolkit` (layer-shell, seat, data-device, `wl_shm` ARGB8888 — premultiplied, matching the renderer), `ashpd` for portals, same calloop loop.

## 5. Windows outline (future, needs spike)

- Overlay: `WS_EX_LAYERED | WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE` popup; `UpdateLayeredWindow` with a premultiplied **BGRA** DIB (swizzle the renderer's RGBA).
- Drop target: OLE `RegisterDragDrop` + `IDropTarget` (`DragEnter/DragOver(grfKeyState, pt)/DragLeave/Drop`), per-position accept via `*pdwEffect = DROPEFFECT_COPY | DROPEFFECT_NONE`; Explorer provides `CF_HDROP`.
- Trigger: `WH_KEYBOARD_LL` + `WH_MOUSE_LL` hooks (no admin rights; UIPI hides elevated windows' input) — Shift pressed while the left button is held and the pointer moved beyond `SM_CXDRAG` → show overlay; if no `DragEnter` follows before button release, hide.
- Risks: false positives on Shift+drag-select (visual flash only), layered-window hit-testing vs. `WindowFromPoint`. Spike W-Win-1.

## 6. macOS outline (future, needs spike)

- Overlay: borderless non-activating `NSPanel`, level `.popUpMenu`, `isOpaque = false`, clear background, `canJoinAllSpaces | fullScreenAuxiliary`; contents via `CALayer` with a `CGImage` from the premultiplied RGBA buffer.
- Drop target: `registerForDraggedTypes([.fileURL])`, `draggingEntered/Updated/Exited`, `performDragOperation`; per-position `NSDragOperation.copy / []`.
- Drag detection: global monitor for `.leftMouseDragged` + `NSPasteboard(name: .drag).changeCount`; trigger by polling `NSEvent.modifierFlags` while dragging (avoids requesting keyboard-monitoring permission). Tangerine demonstrates the interaction is achievable on macOS.
- Spike W-Mac-1.

## 7. `Platform` trait (normative)

```rust
pub struct PhysPoint { pub x: i32, pub y: i32 }
pub struct RectPx { pub x: i32, pub y: i32, pub w: u32, pub h: u32 }
pub enum ShapePx { Circle { cx: f32, cy: f32, r: f32 }, Polygon(SmallVec<[(f32, f32); 6]>) }
pub struct Monitor { pub id: u32, pub name: String, pub rect: RectPx, pub refresh_mhz: u32, pub primary: bool }
pub struct OverlayInfo { pub origin: PhysPoint, pub size: (u32, u32), pub scale: f32, pub monitor: Monitor, pub translucent: bool }
pub struct FrameRef<'a> { pub data: &'a [u8], pub width: u32, pub height: u32, pub stride: usize } // premultiplied RGBA8
pub struct SourceInfo { pub app: Option<String>, pub pid: Option<u32> }

pub enum PlatformEvent {
    DragArmed { source: SourceInfo },
    DragDisarmed,
    Trigger { at: PhysPoint },
    TriggerReleased,
    DndEnter { types: Vec<String> },
    DndPosition { at: PhysPoint },
    DndLeave { drag_ended: bool },                   // true: release on a reject zone, Esc, or source gone
    DndDrop,
    Payload { mime: String, result: Result<Vec<u8>, PayloadError> },
    PointerMoved { at: PhysPoint },                 // Invoke mode
    Button { button: u8, pressed: bool, at: PhysPoint },
    Key { key: Key, pressed: bool },                 // Invoke mode; Key = Left/Right/Up/Down/Tab/ShiftTab/Enter/Space/Backspace/Escape/Digit(u8)
    MonitorsChanged,
    SettingsChanged,
    ConnectionLost,
}

pub trait Platform {
    fn name(&self) -> &'static str;
    fn capabilities(&self) -> &Capabilities;
    #[cfg(unix)] fn poll_fd(&self) -> std::os::fd::RawFd;   // registered with calloop by the app
    fn dispatch_pending(&mut self, out: &mut Vec<PlatformEvent>) -> Result<(), PlatformError>;
    fn monitors(&self) -> &[Monitor];
    fn scale(&self) -> f32;
    fn reduced_motion(&self) -> Option<bool>;
    fn pointer_position(&mut self) -> Result<PhysPoint, PlatformError>;

    fn prepare_overlay(&mut self, at: PhysPoint) -> Result<OverlayInfo, PlatformError>; // configure + buffers, unmapped
    fn map_overlay(&mut self) -> Result<(), PlatformError>;                              // raise + map
    fn present(&mut self, frame: FrameRef<'_>, damage: &[RectPx]) -> Result<(), PlatformError>;
    fn set_input_region(&mut self, shapes: Option<&[ShapePx]>) -> Result<(), PlatformError>; // None = whole overlay
    fn hide_overlay(&mut self);
    fn release_buffers(&mut self);

    fn dnd_reply(&mut self, accept: bool);
    fn request_payload(&mut self, mime: &str);
    fn dnd_finish(&mut self, success: bool);

    fn grab_input(&mut self) -> Result<(), PlatformError>;
    fn ungrab_input(&mut self);

    fn set_clipboard(&mut self, content: ClipboardContent) -> Result<(), PlatformError>;
    fn flush(&mut self) -> Result<(), PlatformError>;

    /// Backend-internal timers (payload timeout, disarm watchdog, poll-mode trigger detection).
    /// The app arms a calloop timer for the returned instant and calls `on_deadline` when it fires.
    fn next_deadline(&self) -> Option<std::time::Instant>;
    fn on_deadline(&mut self, now: std::time::Instant, out: &mut Vec<PlatformEvent>);
}

pub fn connect_default() -> Result<Box<dyn Platform>, PlatformError>;   // picks X11 if $DISPLAY and session type allow
```

`ClipboardContent { uris: Vec<String>, local_paths: Vec<PathBuf>, operation: ClipOp::Copy }`. `PayloadError = Refused | Timeout | TooLarge | Protocol(String)`. `PlatformError` is a `thiserror` enum wrapping connection/reply errors.

## 8. File-manager integration (Invoke mode)

| File manager | Mechanism | File (installed by packaging, M6) |
|--------------|-----------|-----------------------------------|
| **Nemo** | Nemo action | `~/.local/share/nemo/actions/radivine.nemo_action` |
| Nautilus | Script | `~/.local/share/nautilus/scripts/Radivine` |
| Caja | Caja action (same format family as Nemo actions) | `~/.local/share/caja/actions/` |
| Thunar | Custom action (`uca.xml`) | documented, user-installed |
| Dolphin | Service menu | `~/.local/share/kio/servicemenus/radivine.desktop` |

Nemo action content:

```ini
[Nemo Action]
Name=Radivine…
Comment=Open the Radivine action wheel for the selection
Exec=radivine invoke %U
Icon-Name=radivine
Selection=notnone
Extensions=any;
```

## 9. Concentric Layout X11 Overlay & Input-Shape Invariants (`v0.3.0-rc.1`)

- **Single Overlay Window (`T-X111`):** Both `Branching` and `Concentric` modes render into the single pre-created full-monitor `_NET_WM_WINDOW_TYPE_DND` overlay window (`0` child subwindows).
- **Default Capture-All Input Mask (`input.passthrough = false`, `T-X112`):** In Drag mode, the default `ShapeInput` region covers the full active monitor so releasing anywhere outside an actionable leaf sector (central hub, inter-ring radial gap, angular separator, outer margin, caption box, or outside the cluster) sends `XdndStatus(accept=0)` and `XdndFinished(success=0)` (`0` accidental native file moves or drops).
- **Opt-In Pass-Through Input Mask (`input.passthrough = true`, `T-X112`):** When explicitly enabled, Concentric `ShapeInput` is a single solid disc of radius `R_reserve(4) + hit_margin = 334 lp` centered at `C_0` unioned with the displaced-anchor approach corridor (`0` annular holes between rings, invariant across submenu open/close) while excluding the fixed bottom caption box (`Cy + 326..388 lp`).
