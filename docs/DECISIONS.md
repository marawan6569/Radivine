# Radivine — Architecture Decision Records

Format: lightweight ADRs. Status values: **Accepted** (decided in planning), **Provisional** (decided, pending a spike result), **Proposed** (needs owner confirmation), **Superseded**.
To change a decision: add a new ADR that supersedes the old one; never silently edit an Accepted ADR's decision.

| ADR | Title | Status |
|-----|-------|--------|
| 001 | Join the drag as an XDND target via a mid-drag overlay | Accepted (Verified M0 S-1) |
| 002 | One monitor-sized overlay window per session renders all wheels | Accepted |
| 003 | CPU rendering with tiny-skia, cosmic-text, resvg | Accepted |
| 004 | Own X11 backend on x11rb (no GUI toolkit for the overlay) | Accepted |
| 005 | Workspace: 5 library crates, 2 apps, 1 dev tool | Accepted |
| 006 | Pure, time-injected wheel engine that outputs a Scene | Accepted |
| 007 | calloop, one UI thread, worker threads, no async runtime | Accepted |
| 008 | Arming via XFixes; edge-triggered Shift via XI2 raw events only while armed | Accepted (Verified M0 S-2) |
| 009 | Always answer `XdndActionPrivate`; never request `DELETE` | Accepted (Verified M0 S-6) |
| 010 | Adjacent child wheels with a bridge sector; single open chain; wheels never move | Accepted |
| 011 | Hover intent: dwell + outer band + intent triangle + grace + hysteresis | Accepted (defaults tunable) |
| 012 | Two-tier context; visibility from Basic context only | Accepted |
| 013 | Trait-object actions, code-declared menu, TOML custom actions; no plugin ABI | Accepted |
| 014 | Previews in a separate egui process | Accepted |
| 015 | Wayland via capabilities and fallbacks, never input emulation | Accepted |
| 016 | Input pass-through with SHAPE input mask; capture-all fallback | Accepted (Verified M0 S-4) |
| 017 | IPC: JSON lines over a Unix socket; CLI with clap | Accepted |
| 018 | TOML configuration; no settings GUI in v1 | Accepted |
| 019 | Bundle Inter font and Lucide icons | Accepted |
| 020 | No telemetry; local logging with tracing | Accepted |
| 021 | M0 Go/No-Go | **Accepted (GO — 2026-10-22)** |
| 022 | Project licence | Proposed |
| 023 | Invoke mode is both the M2 vehicle and a permanent fallback | Accepted |
| 024 | Pre-created overlay; `_NET_WM_WINDOW_TYPE` chosen by S-5 | Accepted (Verified M0 S-5) |

---

## ADR-001 — Join the drag as an XDND target via a mid-drag overlay
**Status:** Accepted (Empirically verified in Spike [`S-1`](file:///home/marwanmohamed/Documents/work/rust/radivine/docs/spikes/S-1.md))
**Context:** During Nemo's drag the pointer is actively grabbed by Nemo's GTK; Radivine must still learn positions, decide on a selection, and receive file URIs, without patching Nemo or privileges.
**Decision:** On trigger, map an override-redirect, ARGB, `XdndAware=5` overlay under the pointer, followed immediately by a **0-pixel `WarpPointer(NONE, NONE, 0, 0, 0, 0, 0, 0)`** (`pointer_nudge = true` by default). Nemo's GTK window cache (`gdkdnd-x11.c`, which tracks Create/Map/Configure/Shape events of root children during drags) retargets to the overlay within **`1.24–2.80 ms`** even with zero user pointer motion; Radivine then implements the XDND v5 target protocol (including unsolicited `XdndStatus` updates when `drop_target()` changes while stationary).
**Alternatives:** steal the pointer grab (impossible: `AlreadyGrabbed`; breaking it cancels the drag); passive observation via XI2/XRecord (drop still lands on the window below → real file operations); `XdndProxy` on root (only for root drops); Nemo extension (no DnD hooks); patching Nemo (out of bounds); XTEST tricks (fragile).
**Consequences:** No grabs held by Radivine in drag mode; navigation driven by `XdndPosition`; zero-motion release on the center hub safely cancels rather than dropping onto an underlying Nemo folder with `GDK_ACTION_MOVE`; no keyboard navigation in drag mode; Escape cancels the whole drag. Feasibility evidence: [`docs/spikes/S-1.md`](file:///home/marwanmohamed/Documents/work/rust/radivine/docs/spikes/S-1.md), [`docs/spikes/S-6.md`](file:///home/marwanmohamed/Documents/work/rust/radivine/docs/spikes/S-6.md).

## ADR-002 — One monitor-sized overlay window per session renders all wheels
**Status:** Accepted
**Context:** Nested wheels must appear side by side with stems across gaps; the overlay must be a single, stable XDND target.
**Decision:** A single override-redirect ARGB window covering the pointer's monitor renders every wheel, stem and caption. Interaction area is limited by the input shape (ADR-016).
**Alternatives:** one window per wheel (each boundary crossing would produce `XdndLeave`/`XdndEnter` churn, gaps between windows would hit underlying apps — accidental drops — and stems couldn't be drawn across gaps; z-order/compositor sync of several windows); a cluster-sized window resized as children open (resize flicker, buffer reallocation mid-animation).
**Consequences:** One coordinate space; simple stems; SHM buffer the size of the monitor (allocated per session, released after idle, ~16.6 MB at 3840×1080); damage-limited updates keep per-frame cost proportional to the cluster, not the monitor.

## ADR-003 — CPU rendering with tiny-skia, cosmic-text, resvg
**Status:** Accepted (re-evaluate if frame budgets are missed, TECH_STACK §5)
**Context:** ≤ ~50 shapes/frame, must be translucent on every platform, low idle memory, fast activation.
**Decision:** Render into a premultiplied RGBA8 pixmap with tiny-skia; labels via cosmic-text (cached masks); icons via resvg (cached masks).
**Alternatives:** wgpu/vello (GPU init latency and memory, alpha-mode variance across drivers incl. proprietary NVIDIA on X11), OpenGL, cairo (C dependency).
**Consequences:** The same pixel buffer can be presented on X11 (SHM), Wayland (`wl_shm`), Windows (`UpdateLayeredWindow`), macOS (`CGImage`); no blur/backdrop effects; benches required in M1b.

## ADR-004 — Own X11 backend on x11rb
**Status:** Accepted
**Context:** Required: override-redirect, ARGB visual, custom XDND replies, XFixes, XI2 raw events, SHAPE input masks, MIT-SHM via memfd.
**Decision:** Implement the X11 backend directly with x11rb (`RustConnection`, pure Rust).
**Alternatives:** winit 0.30 (no positions during external drags, no per-position accept), winit 0.31 beta (unreleased; still no XFixes/XI2/SHAPE), GTK4 (no arbitrary positioning), GTK3 bindings (unmaintained), Xlib/XCB FFI (unsafe, C deps).
**Consequences:** ~1.5–2.5 k lines of protocol code concentrated in `radivine-platform::x11`, with a pure, unit-tested XDND state machine.

## ADR-005 — Workspace structure
**Status:** Accepted
**Decision:** `radivine-wheel`, `radivine-render`, `radivine-core` (context + action API), `radivine-actions`, `radivine-platform`; apps `radivine`, `radivine-viewer`; tool `wheel-sandbox`. Dependency graph per ARCHITECTURE §2.2, enforced in CI.
**Alternatives:** the brief's split of context/actions/core (tight coupling → churn); a single crate (no enforced boundaries; wheel engine would accrete platform code).
**Consequences:** Clear ownership; pure crates testable in isolation; one extra crate (`render`) compared to the brief. Crates that must not depend on each other define their own small value types (`Shape`/`ShapePx`, render vs. platform `RectPx`/`FrameRef`, core vs. platform `SourceInfo` and clipboard types) and the app converts between them — a deliberate, tiny duplication instead of a shared "types" crate (ARCHITECTURE §6).

## ADR-006 — Pure, time-injected wheel engine that outputs a Scene
**Status:** Accepted
**Decision:** `radivine-wheel` has no I/O, reads no clock, depends on `std` + `smallvec`; consumes `MenuTree` + `WheelInput` + `Millis`; emits `WheelEvent`s and a `Scene`.
**Consequences:** Deterministic scenario/property tests; renderer and platform swappable; engine reused by sandbox, X11, future backends.

## ADR-007 — calloop, one UI thread, worker threads
**Status:** Accepted
**Decision:** UI thread runs calloop with the X11 fd, timers, IPC socket, signal source and worker-result channel. Filesystem/D-Bus/process work happens on short-lived worker threads.
**Alternatives:** tokio (unnecessary runtime/threads), hand-rolled `poll` (re-implementing timers/channels).
**Consequences:** No locks on the hot path; the X connection is used from exactly one thread; future Wayland backend (SCTK) uses the same loop.

## ADR-008 — Arming and trigger detection
**Status:** Accepted (Empirically verified in Spike [`S-2`](file:///home/marwanmohamed/Documents/work/rust/radivine/docs/spikes/S-2.md))
**Decision:** Idle: only XFixes selection notifications for `XdndSelection`. Armed when a non-Radivine owner appears while a button is held. While armed: XI2 (announced 2.2; server reports 2.4) raw key/button events on root for master devices (`trigger_to_map_us = 371–547 µs`). Trigger = fresh press of a key in the configured modifier's mapping; pre-held modifier ignored until released; second press toggles the wheel off. Fallback: `QueryPointer` polling while armed (`trigger_to_map_us = 411 µs`).
**Alternatives:** always-on raw key observation (privacy cost, no benefit); `XGrabKey` on Shift (would break Shift for every app); XRecord (legacy, broader observation).
**Consequences:** Zero idle wakeups; no false activation for Shift+drag text selection; minimal input observation.

## ADR-009 — Always `XdndActionPrivate`; never `DELETE`
**Status:** Accepted
**Context:** Shift makes GTK request `MOVE`; XDND moves are completed by the target requesting `DELETE`.
**Decision:** `XdndStatus`/`XdndFinished` always use `XdndActionPrivate` (or `None` when rejecting); the `DELETE` target is never converted.
**Consequences:** Radivine can never cause the source to delete/move a file; GTK maps `Private` to `COPY` for cursor feedback.

## ADR-010 — Adjacent child wheels, bridge sector, single chain
**Status:** Accepted
**Decision:** A child wheel is placed along the opened segment's bisector at distance `r_p + child_gap + r_c` (rotated up to ±72° and/or shrunk to fit), connected by a stem; the child's sector facing the parent is a neutral bridge; items fan out away from the parent. Only one open child per wheel (a single chain); wheels never move once shown.
**Alternatives:** replacing the parent (forbidden by the brief), concentric outer rings (cramped, poor label space, ambiguous depth), free-floating children (unpredictable).
**Consequences:** Predictable geometry; natural pointer path into the child; deep chains rely on rotation near edges.

## ADR-011 — Hover-intent model
**Status:** Accepted (defaults tunable)
**Decision:** Open child by dwell (140 ms) or outer-band push; protect an open child with an intent triangle + 300 ms grace; angular hysteresis 4°; hub closes children after 150 ms; outside never collapses; activation slop 6 lp.
**Consequences:** Forgiving diagonal travel; bounded lingering; all parameters in config; scenario tests lock behaviour.

## ADR-012 — Two-tier context
**Status:** Accepted
**Decision:** Layout (visibility) depends only on the I/O-free BasicContext; enablement may depend on asynchronously gathered FileFacts and can change live without layout changes.
**Consequences:** Instant wheel even on hung network mounts; items may briefly show "Checking…".

## ADR-013 — Action model
**Status:** Accepted
**Decision:** `Arc<dyn Action>` registry; built-in menu declared in Rust (`MenuSpec`); user custom actions and menu overrides in TOML; actions return `Outcome`/`Effect`s executed by the app on the UI thread.
**Alternatives:** dylib plugins (unsafe ABI), WASM host (premature), fully TOML-declared built-ins (no type checking).
**Consequences:** Small API; clear extension path (crates → optional out-of-process protocol later).

## ADR-014 — Separate viewer process
**Status:** Accepted
**Decision:** Full/Head/Tail previews run in `radivine-viewer` (eframe/egui), spawned detached.
**Consequences:** Daemon memory unaffected; crash isolation; viewer start latency acceptable for previews.

## ADR-015 — Wayland strategy
**Status:** Accepted
**Decision:** Represent unsupported features explicitly (`Capabilities`), offer Invoke mode, Drop Shelf and future DE integrations; spike portal-shortcut + layer-shell overlay (W-A) when Wayland work begins. Never use privileged input access or weaken compositor security.
**Consequences:** No drag-mode parity on Wayland in v0.1; honest UX messaging.

## ADR-016 — Input pass-through
**Status:** Accepted (Empirically verified in Spike [`S-4`](file:///home/marwanmohamed/Documents/work/rust/radivine/docs/spikes/S-4.md))
**Decision:** Default `wheel.leave.passthrough = false` (capture-all: unclipped `ShapeInput`, replying `XdndStatus(accept=false, None)` outside the wheels so releasing outside safely cancels), while also supporting `wheel.leave.passthrough = true` (the overlay's `ShapeInput` mask is the 1-bit union of wheel hit discs and corridors via `shape::Mask(SO::SET, SK::INPUT)`, updated dynamically when child wheels open/close). The standalone wheel engine (`radivine-wheel`) always computes `input_region()` independently of this platform policy.
**Behavioral Trade-Off (Documented from S-4):**
- Under `passthrough = false` (capture-all, **safe default**), releasing anywhere outside the wheel while it is visible **always safely cancels the drag**, and pressing `Shift` a second time toggles the wheel off when the user wants to resume a normal Nemo drag.
- Under `passthrough = true`, moving the pointer outside the wheel cluster retargets the drag to Nemo underneath (`XDND_LEAVE` with `drag_ended = false`), and leaving for > `grace_ms` (350 ms) dismisses the wheel. However, if the user moves outside the wheel over a Nemo folder **while still holding `Shift`** and releases the mouse button within `grace_ms` expecting "releasing outside a menu cancels the menu", Nemo executes a **native `MOVE`** into that folder.

## ADR-017 — IPC and CLI
**Status:** Accepted
**Decision:** JSON-lines over `$XDG_RUNTIME_DIR/radivine/radivine.sock` (0600); single instance via socket bind; CLI via `clap` (`daemon`, `invoke`, `status`, `reload`, `dismiss`, `quit`, `doctor`).
**Alternatives:** D-Bus service (heavier, not cross-platform), signals only (no payload).
**Consequences:** Trivial integration for Nemo actions and scripts.

## ADR-018 — Configuration
**Status:** Accepted
**Decision:** TOML files in `$XDG_CONFIG_HOME/radivine/`; invalid config never prevents startup (defaults + warning); reload via IPC.

## ADR-019 — Bundled font and icons
**Status:** Accepted
**Decision:** Bundle Inter (OFL 1.1) and a Lucide subset (ISC) with licence texts; system fonts only as lazy fallback.
**Consequences:** Deterministic rendering and snapshot tests; ~0.5–1 MB size.

## ADR-020 — Privacy and diagnostics
**Status:** Accepted
**Decision:** No telemetry or network access; `tracing` logs locally (stderr, optional file); key data never logged; `radivine doctor` for user-initiated diagnostics.

## ADR-021 — M0 Go/No-Go
**Status:** **Accepted — GO (2026-10-22)**
**Context:** Milestone M0 required executing Spikes S-1 through S-6 on the live Linux Mint 22.3 Cinnamon X11 desktop (`DISPLAY=:1`, Muffin 6.6.3, NVIDIA RTX 3060, GTK 3.24.41, Nemo 6.6.3) before committing to M1–M6 implementation.
**Decision:** **GO** for the X11 XDND drag-mode architecture as designed, incorporating four empirically verified refinements:
1. **Mandatory 0-Pixel `WarpPointer` Nudge (`pointer_nudge = true` by default):** Issuing `WarpPointer(NONE, NONE, 0, 0, 0, 0, 0, 0)` immediately after `MapWindow` (and after `UnmapWindow` on mid-drag toggle-off) causes the X server to synthesize a `MotionNotify` at `(dx=0, dy=0)` without moving the cursor, retargeting GTK3/Nemo in **`1.24–2.80 ms`** and eliminating the zero-motion `Shift`-move hazard ([`S-1.md`](file:///home/marwanmohamed/Documents/work/rust/radivine/docs/spikes/S-1.md), [`S-6.md`](file:///home/marwanmohamed/Documents/work/rust/radivine/docs/spikes/S-6.md)).
2. **Damage-Rectangle Clipped MIT-SHM Uploads Mandatory:** Full-screen `3840×1080` (`16.59 MB`) `shm::PutImage` stalls NVIDIA Xorg for `32.6 ms` at p95, whereas damage-clipped uploads (`800×600` cluster = `358 µs` p50 / `953 µs` p95; `300×300` wheel = `42 µs` p50 / `48 µs` p95) easily meet the `< 1 ms` budget ([`S-5.md`](file:///home/marwanmohamed/Documents/work/rust/radivine/docs/spikes/S-5.md)).
3. **Stationary-Pointer Status & Submenu Hit-Margin Guard:** GTK 3.24 honors unsolicited `XdndStatus` messages with zero pointer movement (`Reject ↔ Accept`); `WHEEL_ENGINE_DESIGN.md` §6.2 is refined so a child wheel's outer `hit_margin` (`r_out < d ≤ hit_radius`) never preempts a point inside its parent wheel's drawn ring (`d_parent ≤ r_out(parent)`).
4. **Source File Safety Verified:** All test files in `/tmp/radivine-m0-test/main/` retained 100% identical SHA-256 checksums, sizes, mtimes, inodes, and paths across all GTK3 harness and real Nemo 6.6.3 runs ([`S-6.md`](file:///home/marwanmohamed/Documents/work/rust/radivine/docs/spikes/S-6.md)).

## ADR-022 — Open-Source License Selection (`MIT OR Apache-2.0`)
**Status:** Accepted
**Decision:** Radivine is dual-licensed under **`MIT OR Apache-2.0`** at the user's or downstream distributor's option, following standard Rust ecosystem convention. Both [`LICENSE-MIT`](file:///home/marwanmohamed/Documents/work/rust/radivine/LICENSE-MIT) and [`LICENSE-APACHE`](file:///home/marwanmohamed/Documents/work/rust/radivine/LICENSE-APACHE) are provided at the repository root, all workspace crates declare `license = "MIT OR Apache-2.0"`, and all third-party Rust dependencies and embedded assets (`Inter` font under `OFL-1.1`, `Lucide` icons under `ISC`) are audited and documented in [`THIRD_PARTY_NOTICES.md`](file:///home/marwanmohamed/Documents/work/rust/radivine/THIRD_PARTY_NOTICES.md).

## ADR-023 — Invoke mode as M2 vehicle and permanent fallback
**Status:** Accepted
**Decision:** `radivine invoke` (CLI/IPC/Nemo action) opens the same wheel at the pointer with click/keyboard selection and Radivine-owned grabs. It is implemented first (M2) to validate the overlay on the real desktop, and remains the fallback/accessibility path and the basis for Wayland/other DEs.

## ADR-024 — Overlay creation and window type
**Status:** Accepted (Empirically verified in Spike [`S-5`](file:///home/marwanmohamed/Documents/work/rust/radivine/docs/spikes/S-5.md))
**Decision:** Pre-create the overlay at startup (unmapped) to minimise activation latency (`trigger_to_map_us = 371–547 µs`); set `_NET_WM_WINDOW_TYPE_DND` (`map_notify = 169 µs`, `unmap_notify = 62 µs`, zero Muffin animations, zero drop shadows, zero focus or `_NET_CLIENT_LIST` changes) and `_NET_WM_BYPASS_COMPOSITOR = 2`.
