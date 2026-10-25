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
| 018 | TOML configuration; settings GUI added in v0.1.1 (`ADR-025`) and v0.2.0 (`ADR-039`) | Accepted (partially superseded by ADR-025, ADR-039) |
| 019 | Bundle Inter font and Lucide icons | Accepted |
| 020 | No telemetry; local logging with tracing | Accepted |
| 021 | M0 Go/No-Go | **Accepted (GO — 2026-10-22)** |
| 022 | Project licence (`MIT OR Apache-2.0`) | Accepted |
| 023 | Invoke mode is both the M2 vehicle and a permanent fallback | Accepted |
| 024 | Pre-created overlay; `_NET_WM_WINDOW_TYPE` chosen by S-5 | Accepted (Verified M0 S-5) |
| 025 | Control Center architecture, systemd user service lifecycle & settings persistence (`v0.1.1`) | Accepted (`v0.1.1`) |
| 026 | Login autostart graphical readiness & dynamic X11 compositor/display synchronization (`v0.1.1`) | Accepted (`v0.1.1`) |
| 027 | (`ADR-C01`) Conversion is an Action capability, not a second action system | Accepted (`v0.2.0`) |
| 028 | (`ADR-C02`) Crate boundaries: `radivine-dbus`, `radivine-convert`, `radivine-convert-image`, `radivine-jobs`, `radivine-worker` | Accepted (Verified in C1–C5, `Q-A`) |
| 029 | (`ADR-C03`) Capability-based backends with data-only descriptors; deterministic routing | Accepted (`v0.2.0`) |
| 030 | (`ADR-C04`) Built-in image backend: pure-Rust decode, `image` / `jpeg-encoder` encoders, `libwebp` for lossy WebP only | Accepted (Verified in C1–C5, `P1`–`P7`) |
| 031 | (`ADR-C05`) `libwebp` linkage: vendored static (`D2`) | Accepted (Verified in C1–C5, `P1`, `P9`) |
| 032 | (`ADR-C06`) Out-of-process, on-demand worker launched in its own systemd cgroup | Accepted (Verified in C2–C5, `P10`, `Q-B`) |
| 033 | (`ADR-C07`) SQLite (WAL) job store, single writer | Accepted (Verified in C2–C5) |
| 034 | (`ADR-C08`) Job/item state machines; retry creates a new attempt; no auto-resume across sessions (`D9`) | Accepted (`v0.2.0`) |
| 035 | (`ADR-C09`) Worker IPC: JSON lines, command + subscription connections, lossless paths, unified frame bounds | Accepted (`v0.2.0`) |
| 036 | (`ADR-C10`) No-overwrite output commit protocol (`D6`) | Accepted (Verified in C1–C5, `P8`) |
| 037 | (`ADR-C11`) Menu: `Convert ▸ Image ▸ target` (`D4`); visibility by extension, progressive enablement by sniffed MIME; `description_for` hook | Accepted (`v0.2.0`) |
| 038 | (`ADR-C12`) Backend-independent options and presets (`QualityIntent`, `SameFormatPolicy`, validated overrides) | Accepted (Verified in C1–C5) |
| 039 | (`ADR-C13`) Configuration: `convert.toml` (worker-authoritative) + `config.toml [jobs]`; daemon untouched | Accepted (`v0.2.0`) |
| 040 | (`ADR-C14`) Advanced Convert hosted by `radivine-control convert` | Accepted (Verified in C4–C5, `Q-C`) |
| 041 | (`ADR-C15`) Worker sends notifications via standalone `radivine-dbus` (`D7`) | Accepted (`v0.2.0`) |
| 042 | (`ADR-C16`) Optional backend framework ships; no optional backend in v0.2.0 (`D3`) | Accepted (`v0.2.0`) |
| 043 | (`ADR-C17`) Animated sources rejected; same-format governed by `SameFormatPolicy`; skips always reported | Accepted (`v0.2.0`) |
| 044 | (`ADR-C18`) Metadata defaults: auto-orient, keep ICC, strip EXIF (`D5`), drop XMP | Accepted (Verified in C1–C5, `P2`, `P5`) |

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

## ADR-025 — Control Center Architecture, Systemd User Service Lifecycle & Settings Persistence (`v0.1.1`)
**Status:** Accepted (`v0.1.1`)
**Context:** In `v0.1.0`, starting the background daemon required running `radivine daemon` in a terminal or manually copying example service files, and customizing `config.toml` or `actions.toml` required manual text editing. Furthermore, empirical inspection of Linux Mint 22.x (Cinnamon on X11) showed that `cinnamon-session` does not activate `graphical-session.target` in `systemd --user` (only `default.target` is active, with `DISPLAY` and `XAUTHORITY` imported via `/etc/X11/Xsession.d/95dbus_update-activation-env`).
**Decision:**
1. **Standalone Control Center (`apps/radivine-control`):** Implement `radivine-control` (`#![forbid(unsafe_code)]`, `eframe`/`egui` `0.37`) as a separate desktop application launched by `/usr/share/applications/radivine.desktop`, reusing `radivine::config`, `radivine::ipc`, `radivine-actions`, and `radivine-platform::x11::doctor` directly, with single-instance window activation via `$XDG_RUNTIME_DIR/radivine/control.sock` and a non-blocking background worker thread.
2. **Systemd User Service Primary Lifecycle (`/usr/lib/systemd/user/radivine.service`):** Install `radivine.service` in `/usr/lib/systemd/user/` (disabled by default on package install) with `WantedBy=default.target` so `systemctl --user enable radivine.service` starts reliably on Cinnamon login. Import the graphical session environment (`DISPLAY`, `XAUTHORITY`, `WAYLAND_DISPLAY`, `XDG_SESSION_TYPE`, `XDG_CURRENT_DESKTOP`, `DBUS_SESSION_BUS_ADDRESS`) before interactive `start`/`restart`, and verify actual IPC socket health (`radivine.sock`) independently of systemd unit state. When `systemd --user` is unavailable, fall back to a detached `process_group(0)` daemon writing to `~/.local/state/radivine/daemon.log` and XDG autostart. Detect and safely resolve duplicate XDG + systemd autostart conflicts without silently deleting user files.
3. **Format-Preserving Atomic Persistence (`toml_edit`):** Edit `config.toml` and `actions.toml` via `toml_edit::DocumentMut` to preserve comments and unknown keys, validate the complete document before writing, detect external modification conflicts, write atomically via a same-directory `.tmp` file + `rename`, and trigger non-disruptive hot reload (`IpcRequest::Reload`).
4. **System Tray Deferral:** Defer `StatusNotifierItem` / `libappindicator3` system tray integration in `v0.1.1` to avoid introducing a competing GTK3 `glib::MainLoop` alongside `winit`/`eframe` or native C shared-library dependencies; provide explicit `"Close Control Center (daemon keeps running)"` vs `"Stop Daemon"` controls in `radivine-control` ([`CONTROL_CENTER_DESIGN.md`](CONTROL_CENTER_DESIGN.md)).

## ADR-026 — Login Autostart Graphical Readiness & Dynamic X11 Compositor/Display Synchronization (`v0.1.1`)
**Status:** Accepted (`v0.1.1`)
**Context:** During real Linux Mint 22.3 Cinnamon logout/login testing with `radivine.service` enabled (`WantedBy=default.target`), `/etc/X11/Xsession.d/95dbus_update-activation-env` imports `DISPLAY` and `XAUTHORITY` into `systemd --user` ~700 ms before `cinnamon --replace` (Muffin, the window manager and X11 compositor) claims `_NET_WM_CM_S0` and ~140–870 ms before `csd-xsettings` and `csd-xrandr` finish applying DPI/theme/monitor properties. Previously, `X11Platform::connect()` probed `_NET_WM_CM_S0` only once at process startup, permanently caching `compositor_active = false` (`translucent = false`) for the lifetime of the login session. When the user later summoned the radial wheel, `SessionController` rendered with `opaque_fallback = true` (filling rectangular damage boxes with solid opaque `ring_fill`) and applied an XShape `ShapeBounding` clip derived from `ws.input_region()` (which included invisible pointer hit-slop corridors and was not updated on `HoverChanged`), causing severe visual distortion until the user manually restarted the daemon after Muffin was running. Additionally, during logout while LightDM briefly owned `:0`, `radivine.service` (`Restart=on-failure`) could exhaust default systemd burst start limits.
**Decision:**
1. **Dynamic Runtime Compositor & Display Tracking (`X11Platform`):** Subscribe `overlay.helper_window` to `XfixesSelectionNotify` for both `_NET_WM_CM_S0` and `_XSETTINGS_S0` (plus `PROPERTY_CHANGE` on the `_XSETTINGS_S0` owner window), and re-verify `refresh_compositor_state()` and `refresh_display_settings()` inside `prepare_overlay()` before every wheel summon and in `sync_graphical_state()` on status queries. Whenever `_NET_WM_CM_S0` transitions from unowned to owned, immediately reset `ShapeBounding` to unclipped, re-assert `_NET_WM_BYPASS_COMPOSITOR = 2`, clear the X11 window area to transparent (`0x00000000`) on map, and switch `OverlayInfo::translucent` to `true` without requiring a daemon restart.
2. **Bounded Startup Compositor Readiness Wait & Login Retry:** When launched by `systemd --user` (`RADIVINE_SYSTEMD_SERVICE=1` or parent `comm == "systemd"` on a real display), retry transient X11 connection failures during display-manager handover and perform a bounded poll (up to `2500 ms` default, `25 ms` interval) for `_NET_WM_CM_S0` ownership before finalizing the initial `DoctorReport`.
3. **Visual-Only `ShapeBounding` in True No-Compositor Environments:** When running in a genuine uncomposited X11 environment (`!translucent`), compute `ShapeBounding` strictly from visible `scene.wheels` disks and `scene.caption` (never from invisible pointer hit-slop corridors in `ws.input_region()`), update `ShapeBounding` on `HoverChanged` / `SubmenuOpened` / `SubmenuClosed`, and explicitly clear `ShapeBounding` (`set_bounding_region(None)`) whenever `translucent` becomes `true`.
4. **Systemd Service Hardening & Graphical Readiness Telemetry:** Add `After=default.target graphical-session-pre.target dbus.socket`, `StartLimitIntervalSec=0`, and `Environment=RADIVINE_SYSTEMD_SERVICE=1` to `radivine.service`, and expose `graphical_ready`, `compositor_active`, `translucent_overlay`, and `monitor_count` in `DaemonStatus` and the Control Center Dashboard/Diagnostics views.

---

## v0.2.0 Architecture Decisions (`ADR-027` through `ADR-044`, promoted from `ADR-C01`–`ADR-C18`)

> [!NOTE]
> `ADR-027` through `ADR-044` (incorporating owner decisions `D1`–`D9` and all empirical verification spikes `P1`–`P10` and `Q-A`–`Q-D`) have been implemented and verified across Milestones C1 through C5 ([`C1.md`](file:///home/marwanmohamed/Documents/work/rust/radivine/docs/test-reports/C1.md), [`C2.md`](file:///home/marwanmohamed/Documents/work/rust/radivine/docs/test-reports/C2.md), [`C3.md`](file:///home/marwanmohamed/Documents/work/rust/radivine/docs/test-reports/C3.md), [`C4.md`](file:///home/marwanmohamed/Documents/work/rust/radivine/docs/test-reports/C4.md), [`C5.md`](file:///home/marwanmohamed/Documents/work/rust/radivine/docs/test-reports/C5.md)).

## ADR-027 (`ADR-C01`) — Conversion is an Action capability, not a second action system
**Status:** Accepted (`v0.2.0`)
**Context:** Conversion must integrate with the existing Action System without introducing a competing interaction model or blocking the wheel daemon.
**Decision:** Quick Convert targets (`PNG`, `JPEG`, `WebP`) and `Advanced…` are ordinary `impl Action` types in `radivine-actions::convert`, placed in `default_menu()` via `MenuSpec`. `run()` submits a durable job (or spawns the Advanced Convert window) and returns quickly.
**Alternatives:** (a) Dedicated conversion-mode wheel (competing system, rejected). (b) Converting inside `Action::run` on the daemon `Dispatcher` (blocks Dispatcher slots, dies with the daemon, inflates daemon RSS; rejected).
**Consequences:** Reuses existing menu overrides, ordering, `"More…"` overflow, enablement re-check, and error notifications.

## ADR-028 (`ADR-C02`) — Crate boundaries: `radivine-dbus`, `radivine-convert`, `radivine-convert-image`, `radivine-jobs`, `radivine-worker`
**Status:** Accepted (Verified in C1–C5, `Q-A`)
**Decision:**
- `radivine-dbus`: pure-Rust D-Bus notification and `FileManager1` client extracted from `crates/radivine-actions/src/dbus.rs`, zero Radivine dependencies, re-exported by `radivine-actions::dbus`.
- `radivine-convert`: codec-free, desktop-agnostic conversion core (`#![forbid(unsafe_code)]`, zero Radivine crate dependencies), providing `CapabilityCatalog::default_builtin()` so the daemon never links `radivine-convert-image`.
- `radivine-convert-image`: built-in image backend (`#![forbid(unsafe_code)]`), delegating lossy WebP encoding to the isolated 2-function FFI crate `ffi/radivine-webp` (`Q-D`).
- `radivine-jobs`: durable job contract, wire protocol, `JobsClient`, `WorkerLauncher`, `JobsSettings`.
- `apps/radivine-worker`: SQLite WAL store, scheduler, resource governor, handlers, IPC server; depends on `radivine-jobs`, `radivine-convert`, `radivine-convert-image`, and `radivine-dbus` — zero dependency on `radivine-actions`, `radivine-core`, or `radivine-wheel`.
**Consequences:** Clean dependency graph enforced by `./scripts/check-dep-graph.sh` in CI (`X7`). Verified in C3 (`Q-A`): `radivine` daemon stripped size delta is `+129.8 KiB` (`<= +300 KiB` budget) and RSS delta is `+0.58 MiB` (`< +1.0 MiB` budget).

## ADR-029 (`ADR-C03`) — Capability-based backends with data-only descriptors and deterministic routing
**Status:** Accepted (`v0.2.0`)
**Decision:** Backends publish a data-only `BackendDescriptor` (capabilities per `(from, to)`, `FeatureSet`, `max_pixels`, `BackendKind::BuiltIn | ExternalProgram(ProbeSpec)`). Discovery produces `Availability`. `CapabilityCatalog::route` selects deterministically by availability → feature match → `max_pixels` → user preference → `default_priority` → `BackendId`. Implementations (`ConversionBackend`) are instantiated only by the worker (or CLI/test harnesses).
**Consequences:** Three-way availability (`Available`, `RequiresOptional`, `Unsupported`) is first-class and deterministic.

## ADR-030 (`ADR-C04`) — Built-in image backend: pure-Rust decode, `image` / `jpeg-encoder` encoders, `libwebp` for lossy WebP only
**Status:** Accepted (Verified in C1–C5, `P1`–`P7`, `Q-D`)
**Decision:**
- Decode PNG, JPEG, and WebP with the pure-Rust `image` crate (`png 0.18`, `zune-jpeg 0.5`, `image-webp 0.2`) with explicit `image::Limits` (`P3`, `P4`).
- Encode PNG with `image::codecs::png::PngEncoder` and JPEG with `jpeg-encoder 0.7` (`simd` enabled, supporting `4:4:4`, `4:2:2`, `4:2:0` subsampling, progressive scans, optimized Huffman tables, ICC `APP2`, and EXIF `APP1`, verified in `P6`).
- Encode lossless WebP with `image-webp`.
- Encode lossy WebP with native `libwebp` (`sse41`, `avx2`, multi-threaded `thread_level = 1`) behind the minimal 2-function safe wrapper crate `ffi/radivine-webp` (`P1`, `Q-D`), operating only on Radivine-allocated RGBA8/RGB8 pixel buffers.
- Preserve ICC/EXIF via encoder hooks and pure-Rust RIFF `VP8X`/`ICCP`/`EXIF` chunk muxing for WebP (`P2`, `P5`).
- Resize via `image::imageops::FilterType::Lanczos3` (`P7`).
**Consequences:** All untrusted file parsing stays in memory-safe Rust; C code (`libwebp`) only encodes validated pixel buffers.

## ADR-031 (`ADR-C05`) — `libwebp` linkage: vendored static (owner decision `D2`)
**Status:** Accepted (Verified in C1–C5, `P1`, `P9`)
**Decision:** Link `libwebp` statically via vendored C source (`libwebp-sys 0.14.4`, bundling `libwebp 1.6.0 >= 1.3.2`, `P1`), audited with `cargo deny` and `cargo audit` (`P9`) and documented in `THIRD_PARTY_NOTICES.md` and `debian/copyright`.
**Consequences:** Self-contained `.deb` package with no new shared library `Depends` beyond `libc6` and `libgcc-s1`.

## ADR-032 (`ADR-C06`) — Out-of-process, on-demand worker launched in its own systemd cgroup
**Status:** Accepted (Verified in C2–C5, `P10`, `Q-B`)
**Decision:** Single-instance `radivine-worker` (`flock(worker.lock, LOCK_EX)` + `worker.sock`). Launched via `systemctl --user start --no-block radivine-worker.service` (static unit with `After=default.target dbus.socket`, `NoNewPrivileges=yes`, `PrivateTmp=yes`, `ProtectSystem=strict`, `ReadWritePaths=%h %t /tmp -/media -/mnt -/run/media`, `ProtectControlGroups=yes`, `RestrictSUIDSGID=yes`, `LockPersonality=yes`, verified in `Q-B`), falling back to `systemd-run --user --collect --unit=radivine-worker-transient -- radivine-worker serve` when `systemd --user` is active but the static unit is not installed, and falling back to detached `process_group(0)` only when no `systemd --user` manager is running (`P10`). Before idle exit (60 s), unlinks `worker.sock` and drains `accept()` non-blockingly while holding `worker.lock`. Watchdog timeouts use lock-free-I/O `Mutex<ItemPhase>` + `CommitPermit`, bounded ≤ 5 s drain, and `exit(75)` holding `worker.lock` (`O_CLOEXEC`).

## ADR-033 (`ADR-C07`) — SQLite (WAL) job store, single writer
**Status:** Accepted (Verified in C2–C5)
**Decision:** `rusqlite 0.34.0` with `bundled`, WAL mode, single writer (`radivine-worker` serialized by `worker.lock`), schema versioning, retention pruning, at `$XDG_STATE_HOME/radivine/jobs.sqlite3`. `Committing` journal updates and recovery updates always use `PRAGMA synchronous = FULL` before `commit_no_replace`. `PRAGMA wal_checkpoint(TRUNCATE)` runs on startup after recovery and before clean idle exit.

## ADR-034 (`ADR-C08`) — Job/item state machines; retry creates a new attempt; no auto-resume across sessions (`D9`)
**Status:** Accepted (`v0.2.0`, confirmed via `D9`)
**Decision:** Job states: `Queued`, `Preparing`, `Running`, `Cancelling`, `Completed`, `CompletedWithErrors`, `Failed`, `Cancelled`, `Interrupted`. Item states: `Pending`, `Inspecting`, `Running`, `Committing`, `Succeeded`, `Failed`, `Skipped`, `Cancelled`, `Interrupted`. Retry creates a new job linked by `retry_of` reusing the `jobs.resolved` snapshot. Mid-file conversion is restarted from scratch, never resumed. Queued jobs survive a worker crash within the same `boot_id`, and transition to `Interrupted` after logout (`SIGTERM`) or reboot (`D9`).

## ADR-035 (`ADR-C09`) — Worker IPC: JSON lines, command + subscription connections, lossless paths, unified frame bounds
**Status:** Accepted (`v0.2.0`)
**Decision:** JSON lines over `0600` `$XDG_RUNTIME_DIR/radivine/worker.sock` with `SO_PEERCRED` UID check, lossless `WirePath` encoding, `16 MiB` max `submit` request frame, `64 KiB` max non-`submit` request frame, `4 MiB` max command response frame (`get` paged at ≤ 200 items), `64 KiB` max subscription event frame (≤ 10 Hz progress coalescing), and `1..=10,000` items per job (`≤ 4,096` bytes per path).

## ADR-036 (`ADR-C10`) — No-overwrite output commit protocol (`D6`)
**Status:** Accepted (Verified in C1–C5, `P8`)
**Decision:**
- Sources opened `O_RDONLY | O_CLOEXEC` (following user-selected symlinks to regular files, verified `S_ISREG` via `fstat`, re-verified after decode).
- All destination/staging/temp operations relative to opened directory fds with strict `O_NOFOLLOW` / `AT_SYMLINK_NOFOLLOW`.
- Private per-item staging directory `.rvstage-<job>-<rand16>` (`0700`) with two-phase journaling (`StageIntent` before `mkdirat`; `mkdirat` + `fsync(dest_dir)` + `fstat` → `StageCreated` before temp creation).
- Canonical temp files `item-<idx>-<rand16>.part` (`0600`) opened `O_RDWR | O_CREAT | O_EXCL | O_NOFOLLOW | O_CLOEXEC`, journaled via `TempCreated`, `fsync`ed, and re-validated via `O_RDWR` header parse before `acquire_commit_permit`.
- Two-rung atomic commit ladder only (`P8`): Rung A (`renameat_with(NOREPLACE)` → `fsync(stage_dir)` → `fsync(dest_dir)`) → Rung B (`linkat` → `fsync(dest_dir)` → `unlinkat` → `fsync(stage_dir)`) → `ConversionError::FilesystemUnsupported` (no non-atomic copy fallback). Collision suffix `stem (1).ext` .. `stem (999).ext` (`D6`).
- Safe cleanup (`cleanup_temp_safely`) verifies `worker.lock`, regexes, `(stage_dev, stage_ino)` + `0700`/UID namespace exclusivity, and `(temp_dev, temp_ino)` before `unlinkat` + `fsync(stage_dir)`; unverified/unjournaled entries are preserved untouched (`OrphanedTempPreserved`).

## ADR-037 (`ADR-C11`) — Menu: `Convert ▸ Image ▸ target` (`D4`); visibility by extension, progressive enablement by sniffed MIME; `description_for` hook
**Status:** Accepted (`v0.2.0`, confirmed via `D4`)
**Decision:** Insert `Convert ▸ Image ▸ {PNG, JPEG, WebP, Advanced…}` at root index 1 in `default_menu()`. Visibility uses file extensions in `BasicContext` + static `CapabilityCatalog` snapshot (zero I/O). Progressive enablement enables as soon as the first enriched item confirms ≥ 1 convertible image (`confirmed > 0`). Add default method `Action::description_for(&BasicContext)` (`E1`), `FileFacts::is_uninspected()` (`E2`), and lossless `OsStr` percent-encoding in `local_path_to_file_uri` / `Commands::Invoke` (`E8`).

## ADR-038 (`ADR-C12`) — Backend-independent options and presets (`QualityIntent`, `SameFormatPolicy`, validated overrides)
**Status:** Accepted (Verified in C1–C5, `P6`, `P7`)
**Decision:** Express presets as `QualityIntent` (`Lossless`, `Maximum`, `High`, `Balanced`, `Compact`, `Custom(u8)`) + `ResizeSpec` + optional per-format overrides, and include `SameFormatPolicy` (`Skip` for Quick Convert, `Reencode` for Advanced Convert) in `ConversionOptions`. Validate options purely against `FormatTraits`.

## ADR-039 (`ADR-C13`) — Configuration ownership: `convert.toml` + `config.toml [jobs]`; daemon untouched
**Status:** Accepted (`v0.2.0`)
**Decision:** `convert.toml` is owned by `radivine-convert`, resolved authoritatively at runtime only by `radivine-worker` during `Preparing` (snapshotted into `jobs.resolved`), and edited by `radivine-control`. `config.toml [jobs]` is owned by `radivine-jobs`. The wheel daemon (`radivine`) reads zero conversion configuration. Alongside `ADR-025`, this supersedes the v0.1 scope note in `ADR-018` ("no settings GUI in v1").

## ADR-040 (`ADR-C14`) — Advanced Convert hosted by `radivine-control convert`
**Status:** Accepted (Verified in C4–C5, `Q-C`)
**Decision:** `radivine-control convert [--preset ID] [--target FMT] [--stdin] [-- PATHS…]` runs as a separate process that bypasses `ControlInstanceLock::acquire_or_notify_existing()` so multiple Advanced Convert dialogs can coexist with the Settings/Jobs window, spawned in a transient user cgroup scope when `systemd --user` is active, reading bounded `WirePath` lines (`≤ 10,000` lines / `≤ 16 MiB`) and performing header-only `ConversionEngine::inspect` on a background thread (`4.91 ms` cold-start to first frame, well under the `< 350 ms` target).

## ADR-041 (`ADR-C15`) — Worker sends notifications via standalone `radivine-dbus` (`D7`)
**Status:** Accepted (`v0.2.0`, confirmed via `D7`)
**Decision:** Extract `crates/radivine-actions/src/dbus.rs` into `crates/radivine-dbus` (`#![forbid(unsafe_code)]`, depending only on `thiserror`) and re-export `pub use radivine_dbus as dbus;` in `radivine-actions`. `radivine-worker` sends one completion notification per job (`notify = "always"` default, `D7`) via `radivine_dbus::call_desktop_notify`.

## ADR-042 (`ADR-C16`) — Optional backend framework ships; no optional backend in v0.2.0 (`D3`)
**Status:** Accepted (`v0.2.0`, confirmed via `D3`)
**Decision:** Ship the external-program discovery and routing framework (`BackendKind::ExternalProgram(ProbeSpec)`, `probe_all`, `Availability::MissingDependency`, `TargetStatus::RequiresOptional`) in v0.2.0 tested with a stub program, while deferring `external.vips` implementation to v0.2.x (`D3`).

## ADR-043 (`ADR-C17`) — Animated sources rejected; same-format governed by `SameFormatPolicy`; skips always reported
**Status:** Accepted (`v0.2.0`)
**Decision:** Reject animated APNG/WebP sources with `ConversionError::UnsupportedFeature(Animation)`. Govern same-format sources via `SameFormatPolicy` (`Skip` with `AlreadyTargetFormat` in Quick Convert; `Reencode` with numbered output in Advanced Convert). Never ignore skipped files silently.

## ADR-044 (`ADR-C18`) — Metadata defaults: auto-orient, keep ICC, strip EXIF (`D5`), drop XMP
**Status:** Accepted (Verified in C1–C5, `P2`, `P5`)
**Decision:** Default to `OrientationPolicy::AutoOrient` (pixels rotated/flipped, EXIF orientation tag normalized to `1` when EXIF is preserved), `IccPolicy::Preserve` (byte-exact when backend reports `ICC_EMBED`, otherwise `Warning::IccNotPreserved`), `ExifPolicy::Strip` (`D5`), XMP/IPTC dropped, new mtime by default, and output mode `0o600` staging promoted via atomic commit.

---

## v0.3.0-rc.1 Architecture Decisions (`ADR-045` / `ADR-NW-001`)

## ADR-045 (`ADR-NW-001`) — Optional Concentric (Nested Rings) Wheel Layout & Non-Actionable Safety Envelope (`v0.3.0-rc.1`)
**Status:** Proposed (Pending Final Owner Desktop Acceptance — Published in `v0.3.0-rc.1` Public Pre-release; `ADR-010` historical default preserved)
**Context:** `ADR-010` selected Branching / Vine offset child wheels for `v0.1.0`–`v0.2.0` and recorded Concentric radial menus for future evaluation. Users with deeper submenus (`File Preview`, `Convert ▸ Image`, `Custom Actions`) requested an optional single-center concentric rings layout (`wheel.layout = "concentric"`) that eliminates multi-wheel screen travel while preserving 100% of `Branching` behavior as the default (`wheel.layout = "branching"`).
**Decision:**
1. **Default Preserved (`wheel.layout = "branching"`):** Missing or invalid `wheel.layout` defaults to `Branching` with byte-identical rendering and historical trace parity (`T-REN5`, `T-PERF1`).
2. **Concentric Annular Geometry (`wheel.layout = "concentric"`):** All open submenu levels `L in 0..D` share a single immobile root center `C_0` (`R_hub = 50 lp`, root ring `50..118 lp`, outer ring `L >= 1` inner radius `118 + 66·(L-1) lp`, thickness `60 lp`, inter-ring gap `6 lp`, minimum sector sweep `30°` with overflow pagination).
3. **Reserved 4-Ring + Fixed Caption Viewport Contract (`668 × 742` Logical Pixels):** Before mapping the overlay, Concentric mode reserves 4 rings (`R_reserve(4) = 326 lp`, `±334 lp` horizontal/top with `8 lp` screen margin) plus the fixed bottom caption box (`Cy + 326..388 lp`, `+408 lp` bottom extent with `20 lp` margin), requiring a minimum workarea of **`668 × 742` logical pixels**. Viewports meeting `668 × 742` logical pixels (including `1366×768@1x` and `1920×1080@1.25x`) activate Concentric; smaller logical viewports (`1280×720@1x`, `1920×1080@1.5x`) automatically fall back to `Branching` (`LayoutFallbackReason::InsufficientViewportSpace`) for that session without mutating `config.toml`.
4. **Strict Non-Actionable Drop & Click Safety Envelope (`T-SAFE1`–`T-SAFE10`):** Central hub (`r < 50 lp`), inter-ring radial gaps (`6 lp`), angular separator gaps, outer ring margin (`r > R_out(D)`), displaced-center approach corridor, protected pending-switch sibling sectors (`90 ms` outer-ring steering guard), submenu parent sectors on drag release, and the fixed bottom caption box never arm or commit an action (`hover_action = None`, `XdndStatus(accept=0)`, `XdndFinished(success=0)`), and `DndDrop` re-evaluates the final pointer coordinate before committing.
5. **Portable Arabic Font Fallback in Packaging:** `packaging/debian/control` declares `Depends: ..., fonts-dejavu-core, libfontconfig1` and `Recommends: ..., fonts-noto-core` so `radivine-render` (`/usr/share/fonts/truetype/dejavu/DejaVuSans*.ttf`) and `radivine-control`/`radivine-viewer` (`egui_system_font_provider`) render shaped Arabic text out of the box on clean Debian/Ubuntu/Linux Mint systems.


