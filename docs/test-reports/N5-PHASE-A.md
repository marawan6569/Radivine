# Milestone N5 Phase A Verification Report — Priority Safety & Isolated X11 Acceptance (`v0.3.0`)

**Date:** 2026-04-20
**Status:** **N5 PHASE A COMPLETE** (Isolated `Xvfb` `T-X111`–`T-X118` Verified; `668×742 lp` Geometry Contract Verified; Full Workspace Regression Gate Passed; Stopping for Owner Phase B Authorization)
**Scope:** Authorized **N5 Phase A ONLY** — isolated `Xvfb` X11 safety & protocol verification (`T-X111`–`T-X118`), `668×742 lp` geometry discrepancy verification (`T-GEO4`), recording Owner Manual Positive Smoke Test evidence, preparing the owner's disposable-file desktop acceptance checklist, and running full workspace regression and live `Xvfb` timing measurements.

---

## 1. Executive Summary

1. **Isolated X11 (`Xvfb`) Execution Verified (`T-X111`–`T-X118`):**
   - `Xvfb` (`/usr/bin/Xvfb`) and `xvfb-run` (`/usr/bin/xvfb-run`) were verified on the host and executed strictly inside an isolated `1920×1080×24 +extension COMPOSITE +extension RANDR` virtual X server (`DISPLAY=:99`+) with isolated temporary `XDG_*` directories and `DBUS_SESSION_BUS_ADDRESS="unix:path=$tmpdir/no-dbus"`.
   - The live Cinnamon/Nemo session (`DISPLAY=:0`), installed Radivine `v0.2.0` binary, user configuration, Nemo integration, and systemd user services were never touched.
   - All display-gated tests guarded by `radivine_platform::is_isolated_test_display()` were explicitly verified to execute their real X11 bodies (not early-return), emitting structured `LIVE_XVFB_VERIFIED` and `LIVE_XVFB_METRICS` trace lines.
2. **Geometry Discrepancy Resolved (`668×742 lp` vs. `668×668 lp`):**
   - Inspected `crates/radivine-wheel/src/concentric.rs` (`plan_concentric`, lines 294–324): production code already enforces `required_w = 2 * (8 + max(326, 140)) = 668.0 lp` and `required_h = 2 * (8 + 326) + 10 + 64 = 742.0 lp` (`668×742 lp`).
   - The `668×668 lp` figure in `docs/test-reports/N4.md` was a documentation typo that omitted the `+ 10 lp` caption gap and `+ 64 lp` caption height; `docs/test-reports/N4.md` was corrected and both pure-unit and live `Xvfb` tests now explicitly verify `1366×768@1` (`Concentric`), `1280×720@1` (`Branching`, `insufficient_space`), `668×742@1` vs. `668×741@1` / `667×742@1`, and non-clipping of all 4 rings (`Rreserve = 326 lp`) and the `280×64 lp` caption box across all 4 edges and 4 corners.
3. **Real Desktop Smoke Test Evidence Recorded:**
   - Recorded **Owner Manual Positive Smoke Test — PASS** (`Nemo → real file Drag + Shift → Concentric Rings → submenu navigation → successful Action execution`), explicitly scoped as positive smoke evidence rather than complete desktop acceptance.
   - Prepared a concise, self-contained disposable-file verification checklist (§4.2) for owner execution covering reject zones, file identity preservation (`path`, `inode`, `size`, `mtime`, and `SHA-256`), live settings reload, both layouts (`Branching` and `Concentric`), and monitor edge/corner clamping.
4. **Full Workspace Gate & Regression Suites:**
   - All **249 active workspace tests** (`radivine-wheel`, `radivine-render`, `radivine-platform`, `radivine-core`, `radivine-actions`, `radivine-convert`, `radivine-convert-image`, `radivine-webp`, `radivine-jobs`, `radivine-worker`, `radivine-viewer`, `radivine-dbus`, `radivine-control`, and `radivine`) passed under isolated `Xvfb` with `0` failures.

---

## 2. Isolated X11 (`Xvfb`) Execution vs. `FakePlatform` Simulations

### 2.1 Execution Command (Documented Repository Workflow)

```bash
tmpdir="$(mktemp -d)"
trap 'rm -rf "$tmpdir"' EXIT
mkdir -p -m 700 "$tmpdir"/{config,state,data,run}
xvfb-run -a -s "-screen 0 1920x1080x24 +extension COMPOSITE +extension RANDR" \
  env \
    XDG_CONFIG_HOME="$tmpdir/config" \
    XDG_STATE_HOME="$tmpdir/state" \
    XDG_DATA_HOME="$tmpdir/data" \
    XDG_RUNTIME_DIR="$tmpdir/run" \
    DBUS_SESSION_BUS_ADDRESS="unix:path=$tmpdir/no-dbus" \
    RADIVINE_FORCE_ARGB=1 \
    cargo test -p radivine-platform -p radivine -- --test-threads=1 --nocapture
```

### 2.2 Distinction Between Real X11 (`Xvfb`) Tests and `FakePlatform` Simulations

| Test Target / Suite | Execution Backend | Display-Gate Proof | Scope Verified | Result |
|---|---|---|---|---|
| `radivine-platform::x11_tests` (`4` live tests: `x11_ finds_32bit_argb_visual_when_available`, `x11_overlay_lifecycle_create_map_present_unmap_destroy`, `x11_damage_present_benchmark_under_2ms`, `x11_grab_release_100_cycles_zero_stuck_grabs`) | **Real X11 (`Xvfb`)** via `x11rb::rust_connection::RustConnection` + MIT-SHM | Asserts `is_isolated_test_display() == true`; queries real X11 root visual depth `32`, maps/unmaps real X11 window, runs 100 real pointer/keyboard grab/ungrab cycles (`0.12 ms` avg present). | `M1b` / `T-X111` / `T-X116` low-level X11 window, MIT-SHM damage present, and grab hygiene. | **PASS** (`4/4` live + `11/11` structural) |
| `radivine::nested_acceptance::live_xvfb_concentric_end_to_end_xdnd_session_and_compositor_guards` | **Real X11 (`Xvfb`)** + spawned `radivine daemon` + raw X11 client (`x11rb`) | Emits `LIVE_XVFB_VERIFIED: T-X111 T-X112 T-X113 T-X114 T-X115 T-X116 T-X117 T-X118 T-GEO4`; fails hard via `panic!` if `!is_isolated_test_display()` on `:99`+. | Full live X11 wire verification of `T-X111`–`T-X118` and `T-GEO4` (see §2.3). | **PASS** (`3.01 s`) |
| `radivine::nested_acceptance` (Scenarios `1`–`12` deterministic harness) | **`FakePlatform` simulation** (`apps/radivine/src/main.rs` event loop) | Pure in-memory `FakePlatform` driving real `App::handle_event` and `FileFingerprint` (`path`, `ino`, `size`, `mtime`, `sha256`, `bytes`). | Deterministic state machine, `1366×768@1` / `1280×720@1` geometry, 500-iteration fuzz, 200-session soak. | **PASS** (`7/7`) |
| `radivine::m2_acceptance::m2_live_acceptance_suite` | **Real X11 (`Xvfb`)** + `xdotool` + `radivine daemon` | Panics if `!is_isolated_test_display()`; verifies real X11 window map/unmap, `xdotool` keyboard/mouse navigation, and 50 invoke/Escape cycles. | `M2` live X11 invoke/dismiss lifecycle and grab release. | **PASS** (`4.17 s`) |
| `radivine::m3_drag_acceptance::m3_drag_and_drop_acceptance_suite` | **Real X11 (`Xvfb`)** + `xdotool` + `x11rb` XDND wire client | Panics if `!is_isolated_test_display()`; runs 20 trigger→mapped latency trials + 50 mixed drag sessions over real X11 sockets. | `M3` / `T-X113` / `T-X114` live XDND wire protocol and latency. | **PASS** |
| `radivine::m3_drag_acceptance::m3_real_gtk3_drag_retargeting` | **Real X11 (`Xvfb`)** + real **GTK3 (`python3-gi` `Gtk.drag_begin_with_coordinates`)** + `xdotool` | Spawns real GTK3 drag source window under `Xvfb`, holds `Button1` + presses `Shift_L`, verifies GTK3 dynamically retargets active drag onto Radivine's newly mapped overlay window and commits without dropping onto the underlying window. | Real GTK3 / Nemo toolkit XDND retargeting under X11 grab. | **PASS** (`10.22 s` suite) |
| `radivine::m4_actions_acceptance::m4_contextual_actions_acceptance_suite` | **Real X11 (`Xvfb`)** + `radivine daemon` | Panics if `!is_isolated_test_display()`; verifies contextual action execution (`Copy`, `Duplicate`, `New Folder with Selection`, `Archive`, `Trash`) on temporary files. | `M4` end-to-end action execution under X11. | **PASS** (`1.63 s`) |
| `radivine::m5_preview_acceptance` (`3` tests) | **Real X11 (`Xvfb`)** + `radivine daemon` | Panics if `!is_isolated_test_display()`; verifies Quick Look viewer (`Full`, `Head`, `Tail`, binary hex) and VmRSS stability. | `M5` viewer integration and memory bounds. | **PASS** (`2.70 s`) |
| `radivine::v0_1_1_login_autostart_regression` (`xvfb_wire_daemon_dynamically_recovers_when_compositor_and_dpi_initialize_after_startup`) | **Real X11 (`Xvfb`)** + `radivine daemon` | Acquires `_NET_WM_CM_S0` and updates root `RESOURCE_MANAGER` (`Xft.dpi: 144`) after daemon startup; verifies live recovery to ARGB + `1.5×` scale without restart. | Live X11 compositor & DPI late-initialization recovery. | **PASS** (`1.04 s`) |
| `radivine-control::issue_1_clipboard::live_x11_clipboard_owner_coexists_with_external_selection_steal` | **Real X11 (`Xvfb`)** + `x11rb` selection owner | Acquires `CLIPBOARD` selection on live X11 server, verifies `text/uri-list` and `x-special/gnome-copied-files` payload transfer and clean handoff when another X11 client steals ownership. | Live X11 `CLIPBOARD` selection protocol. | **PASS** (`0.13 s`) |

### 2.3 Detailed `T-X111`–`T-X118` Verification Results (`live_xvfb_concentric_end_to_end_xdnd_session_and_compositor_guards`)

All eight X11 safety and protocol requirements in `docs/nested-wheel/TEST_MATRIX.md` §4 were verified against the live `Xvfb` server:

- **`T-X111` — Single ARGB Overlay Window Reuse & Zero Drag Input Grab:**
  - Verified `conn.get_geometry(overlay_win)?.depth == 32` (32-bit ARGB visual).
  - Verified `overlay_window_id` remains identical across root ring `0` and opening/closing child ring `1`, and `conn.query_tree(root)` / `conn.query_tree(overlay_win)` confirm **zero additional X11 windows** are created when child rings open.
  - Verified `conn.grab_keyboard(false, native_drop_win, CURRENT_TIME, ASYNC, ASYNC)` returns `GrabStatus::SUCCESS` during an active Drag session, proving Radivine holds **zero keyboard/pointer grab** in Drag mode while Nemo owns the pointer grab.
- **`T-X112` — `XShape` (`ShapeInput`) Full-Monitor Capture vs. Opt-In Passthrough:**
  - **Default Drag (`input.passthrough = false`):** `conn.shape_get_rectangles(overlay_win, SK::INPUT)` returns `[(0, 0, 1920, 1080)]` (full-monitor capture-all rectangle), preventing any pointer event or drop from reaching underlying windows.
  - **Opt-in Drag (`input.passthrough = true`):** Summoned near the left monitor edge at `(10, 540)` so the wheel center clamps to `(334, 540)`:
    - `shape_get_rectangles(overlay_win, SK::INPUT)` covers the **solid reserved disc** `r = 0..330` around `(334, 540)` (explicitly verified to include the center hub `(334, 540)` and all inter-ring gaps at radii `131`, `197`, and `263`, eliminating internal mouse-exit holes).
    - Covers the summon anchor `(10, 540)` and approach corridor midpoint `(172, 540)`.
    - **Excludes** the detached caption box `(334, 908)` and outside-cluster points `(1200, 540)`.
    - Verified `shape_get_rectangles(overlay_win, SK::INPUT)` bounding extent is unchanged when child ring `1` opens (`Rreserve = 326 lp` is reserved up front).
- **`T-X113` — Bit-Exact `XdndStatus` & `XdndFinished` Wire Negotiation and Zero `DELETE`:**
  - Hovering actionable leaf `[1] Copy` at `(1044, 540)` with source offering `XdndActionCopy` returns `XdndStatus.data32 = [overlay_win, 1, 0, 0, XdndActionPrivate]`.
  - Hovering any non-commit zone (center hub `(960, 540)`, radial gap `(1091, 540)`, angular separator `(1026, 489)`, outer ring margin `(1200, 540)`, caption box `(960, 908)`, or outside cluster `(200, 200)`) — even when the source advertises `XdndActionMove` — returns `XdndStatus.data32 = [overlay_win, 0, 0, 0, 0]` (`accept = 0`, `action = None`).
  - Dropping on `[1] Copy` returns `XdndFinished.data32 = [overlay_win, 1, XdndActionPrivate, 0, 0]`; dropping on any reject zone returns `XdndFinished.data32 = [overlay_win, 0, 0, 0, 0]`.
  - Inspected all `SelectionRequest` events received by the drag source across both commit and reject sessions: **zero `DELETE` target requests** and **zero `XdndActionMove` acknowledgments** ever occurred.
- **`T-X114` — Stale Source Filtering, Zero-Motion Hub Drop, Prefetch Failure Fallback, and `XdndLeave` Re-Entry:**
  - Injected spoofed `XdndPosition` and `XdndDrop` from `0xdead_beef` during an active session owned by `src_win`: ignored with zero `XdndStatus` / `XdndFinished` replies and session state unchanged.
  - Immediate `XdndDrop` at summon center `(960, 540)` with zero pointer motion: rejected (`XdndFinished` `accept = 0`, `last_committed_action = null`).
  - Selection prefetch refusal (`SelectionNotify` with `property = NONE` during `XdndEnter`): daemon stays alive with provisional menu (`session_state = "Opening"`), then cleanly resolves selection on `XdndDrop`.
  - `XdndLeave` followed by re-entry (`XdndEnter` + `XdndPosition`): cleanly resets drag tracking and resumes hit-testing.
- **`T-X115` — Multi-Monitor / Corner Clamping & Live `Xft.dpi` Scaling (`1.0×`, `1.25×`, `1.5×`):**
  - Verified all four corner summons on `1920×1080@1` clamp `root_wheel_center_px` to `(334, 334)` (top-left), `(1586, 334)` (top-right), `(334, 672)` (bottom-left), and `(1586, 672)` (bottom-right), where `672 = 1080 - 8 - 326 - 10 - 64` reserves full space for both the 4-ring cluster (`Rreserve = 326`) and the `64 px` caption box (`10 px` gap, `8 px` edge pad).
  - Updated live X11 root `RESOURCE_MANAGER` to `Xft.dpi: 120` (`1.25×`, logical viewport `1536×864 >= 668×742`): `active_layout = "concentric"`.
  - Updated live X11 root `RESOURCE_MANAGER` to `Xft.dpi: 144` (`1.5×`, logical viewport `1280×720 < 668×742`): `active_layout = "branching"`, `layout_fallback_reason = "insufficient_space"`.
- **`T-X116` — Invoke Mode Keyboard & Mouse Grab Lifecycle:**
  - Verified `radivine invoke` maps the Concentric overlay, processes `xdotool key 1` (opens ring `1`), `BackSpace` (retracts to ring `0`), and `Right` + `Return` (commits `radivine.copy`), unmaps `overlay_win`, and releases all grabs so `conn.grab_pointer(..., native_drop_win, ...)` immediately returns `GrabStatus::SUCCESS`.
- **`T-X117` — Compositor Absence & Mid-Session Compositor Loss Fallback:**
  - Releasing `_NET_WM_CM_S0` before summon transitions daemon status to `compositor_active = false`, `active_layout = "branching"`, `layout_fallback_reason = "no_compositor"`, and applies non-rectangular `ShapeBounding` (`SK::BOUNDING` rectangles `> 1`) so uncomposited X11 shows no black rectangular corners.
  - Releasing `_NET_WM_CM_S0` **mid-drag** while Concentric ring `1` is open safely aborts the Concentric visual state without committing any action or leaking events to `native_drop_win`, and subsequent drags use `branching` fallback until `_NET_WM_CM_S0` returns.
- **`T-X118` — Zero Pass-Through to Underlying `XdndAware` File Manager Window:**
  - Mapped a full-screen (`1920×1080`) `XdndAware` `native_drop_win` directly beneath Radivine's overlay. Across all 7 reject-zone drops (hub, radial gap, angular separator, outer margin, caption box, outside cluster, and zero-motion drop), mid-drag config `reload`, and mid-drag `_NET_WM_CM_S0` loss:
    - `count_xdnd_events(&conn, native_drop_win) == 0` (zero `XdndEnter`, `XdndPosition`, `XdndDrop`, or `ButtonRelease` leaked).
    - `native_drop_dest/` remained completely empty.
    - Source file `FileFingerprint` (`path`, `ino`, `size`, `mtime`, `sha256`, `bytes`) remained bit-for-bit identical before and after every scenario.

---

## 3. Verification of the `668×742 lp` Geometry Contract (`T-GEO4`)

### 3.1 Code Audit (`crates/radivine-wheel/src/concentric.rs`)

Inspected `plan_concentric` in `crates/radivine-wheel/src/concentric.rs` (lines 294–324):

```rust
let r_reserve = concentric_reserved_radius_lp(max_depth); // max_depth = 4 => 326.0 lp
let half_w = CONCENTRIC_EDGE_PAD + r_reserve.max(CONCENTRIC_CAPTION_W * 0.5); // 8.0 + max(326.0, 140.0) = 334.0 lp
let top_extent = CONCENTRIC_EDGE_PAD + r_reserve; // 8.0 + 326.0 = 334.0 lp
let bottom_extent =
    CONCENTRIC_EDGE_PAD + r_reserve + CONCENTRIC_CAPTION_GAP + CONCENTRIC_CAPTION_H; // 8.0 + 326.0 + 10.0 + 64.0 = 408.0 lp
let required_w = 2.0 * half_w; // 668.0 lp
let required_h = top_extent + bottom_extent; // 742.0 lp
```

- **Finding:** Production code in `crates/radivine-wheel/src/concentric.rs` was **already compliant** with the `668×742 lp` contract (`INTERACTION_DESIGN.md` §I2, `TEST_MATRIX.md` `T-GEO4`). The `668×668` number in `docs/test-reports/N4.md` was a report-only error that neglected `CONCENTRIC_CAPTION_GAP (10 lp) + CONCENTRIC_CAPTION_H (64 lp) = 74 lp`.
- **Action Taken:** Corrected `docs/test-reports/N4.md` (§2.1 and §4) and expanded both `apps/radivine/tests/nested_acceptance.rs` and standalone `Xvfb` screen-size tests to lock down every boundary condition.

### 3.2 Confirmed Geometry & Viewport Fallback Matrix

| Viewport / Configuration | Logical Size (`lp`) | Expected Layout | Fallback Reason | Caption & 4-Ring (`Rreserve = 326 lp`) Clipped? | Verification Method |
|---|---|---|---|---|---|
| `1366×768 @ 1.0×` | `1366 × 768` (`>= 668 × 742`) | **`concentric`** | `none` | **No** (`center.y` clamped to `[334, 360]`) | Unit (`concentric_geometry.rs`), `FakePlatform` (`nested_acceptance.rs`), and standalone `xvfb-run -s "-screen 0 1366x768x24"` |
| `1280×720 @ 1.0×` | `1280 × 720` (`720 < 742`) | **`branching`** | `insufficient_space` | **No** (falls back cleanly before mapping) | Unit (`concentric_geometry.rs`), `FakePlatform` (`nested_acceptance.rs`), and standalone `xvfb-run -s "-screen 0 1280x720x24"` |
| `1920×1080 @ 1.25×` (`Xft.dpi: 120`) | `1536 × 864` (`>= 668 × 742`) | **`concentric`** | `none` | **No** | Live `Xvfb` (`T-X115` root `RESOURCE_MANAGER`) + `nested_acceptance.rs` |
| `1920×1080 @ 1.50×` (`Xft.dpi: 144`) | `1280 × 720` (`720 < 742`) | **`branching`** | `insufficient_space` | **No** | Live `Xvfb` (`T-X115` root `RESOURCE_MANAGER`) + `nested_acceptance.rs` |
| `1920×1080 @ 2.00×` | `960 × 540` (`540 < 742`) | **`branching`** | `insufficient_space` | **No** | `nested_acceptance.rs` Scenario 7a |
| `668×742 @ 1.0×` (exact minimum) | `668 × 742` | **`concentric`** | `none` | **No** (`center = (334, 334)`, caption `y = [670, 734]`, pad `8 lp` top/bottom/left/right) | `concentric_geometry.rs` + `nested_acceptance.rs` Scenario 7a |
| `668×741 @ 1.0×` / `667×742 @ 1.0×` | `1 lp` below minimum | **`branching`** | `insufficient_space` | **No** | `concentric_geometry.rs` + `nested_acceptance.rs` Scenario 7a |

---

## 4. Real Desktop Smoke Test Evidence & Owner Disposable-File Checklist

### 4.1 Recorded Owner Smoke Test Evidence

| Test Case | Environment | Path Exercised | Status | Scope Note |
|---|---|---|---|---|
| **Owner Manual Positive Smoke Test** | Live Linux Mint / Cinnamon + Nemo (`v0.2.0` coexistence environment) | `Nemo → real file Drag + Shift → Concentric Rings → submenu navigation → successful Action execution` | **PASS** | Confirms positive real-desktop Nemo drag summon, Concentric ring rendering, child ring navigation, and action execution. Does **not** substitute for the negative/reject-zone and identity-preservation checks below. |

### 4.2 Concise Owner Disposable-File Desktop Acceptance Checklist (For Phase B Sign-Off)

> **Safety Note:** Perform all checks below using **only the disposable `/tmp/radivine-n5-check` directory** created in Step 0. Do not test with user documents.

#### Step 0 — Create Disposable Test Files & Record Baseline Identity (`path`, `inode`, `size`, `mtime`, `SHA-256`)

```bash
rm -rf /tmp/radivine-n5-check
mkdir -p /tmp/radivine-n5-check/src /tmp/radivine-n5-check/drop-target
printf 'radivine-n5-disposable-payload-2026\n' > /tmp/radivine-n5-check/src/sample.txt
cp /usr/share/pixmaps/debian-logo.png /tmp/radivine-n5-check/src/sample.png 2>/dev/null || \
  printf '\x89PNG\r\n\x1a\n' > /tmp/radivine-n5-check/src/sample.png

record_identity() {
  for f in /tmp/radivine-n5-check/src/*; do
    printf '%s | ino=%s | size=%s | mtime=%s | sha256=%s\n' \
      "$f" "$(stat -c '%i' "$f")" "$(stat -c '%s' "$f")" "$(stat -c '%Y.%N' "$f")" "$(sha256sum "$f" | awk '{print $1}')"
  done
  printf 'drop-target-count=%s\n' "$(find /tmp/radivine-n5-check/drop-target -mindepth 1 | wc -l)"
}

record_identity | tee /tmp/radivine-n5-check/baseline.id
```

#### Step 1 — Verify Helper (Run After Every Reject-Zone Release)

```bash
verify_unchanged() {
  record_identity > /tmp/radivine-n5-check/current.id
  diff -u /tmp/radivine-n5-check/baseline.id /tmp/radivine-n5-check/current.id && echo "PASS: File identity (path, inode, size, mtime, SHA-256) and empty drop-target preserved"
}
```

#### Step 2 — Reject-Zone & File-Identity Matrix (Concentric Layout)

Open `/tmp/radivine-n5-check/src` and `/tmp/radivine-n5-check/drop-target` side-by-side in Nemo. Ensure `radivine-control` Wheel Appearance is set to **Concentric Rings**. For each row below, drag `sample.txt` from `src/` over the `drop-target/` window, press `Shift` to summon Radivine, open a child submenu (`Convert` or `Archive`) where applicable, release the mouse button in the specified zone, and run `verify_unchanged`:

| # | Release Zone | Expected Visual / Desktop Behavior | Identity Check (`verify_unchanged`) |
|---|---|---|---|
| **R1** | **Center Hub (`r < 56 lp`)** | Wheel dismisses cleanly; no action runs; file is **not** moved/copied into `drop-target/`. | `diff` empty (`PASS`) |
| **R2** | **Inter-Ring Radial Gap** (between ring `0` and opened ring `1`) | Wheel remains open during hover (`action = None`) and dismisses on release with no action. | `diff` empty (`PASS`) |
| **R3** | **Angular Separator Gap** between adjacent sectors | No sector highlighted; release dismisses with no action. | `diff` empty (`PASS`) |
| **R4** | **Caption Box** (`280×64 lp` below rings) | Release over caption box dismisses with no action. | `diff` empty (`PASS`) |
| **R5** | **Outside Wheel Cluster** (directly over the underlying `drop-target/` Nemo window) | Default `input.passthrough = false` overlay captures the release; **`drop-target/` remains empty** and `sample.txt` keeps identical `path`, `inode`, `size`, `mtime`, and `SHA-256`. | `diff` empty (`PASS`) |
| **R6** | **`Escape` Key Cancel During Active Drag** | Pressing `Escape` dismisses the overlay; releasing the mouse button afterwards does **not** move/copy `sample.txt` into `drop-target/`. | `diff` empty (`PASS`) |

#### Step 3 — Live Settings Reload & Both Layouts (`Concentric` ↔ `Branching`)

1. Toggle **Wheel Appearance** in `radivine-control` from **Concentric Rings** to **Branching Wheel** (without restarting the daemon):
   - Drag `sample.txt` + `Shift`: verify **Branching Wheel** appears on the next session and releasing outside the wheel over `drop-target/` still passes `verify_unchanged`.
2. Toggle back to **Concentric Rings**, summon via `radivine invoke /tmp/radivine-n5-check/src/sample.txt`, and run `radivine reload` from a terminal while a child ring is open:
   - Verify the active session stays stable and the new configuration takes effect on the next session.

#### Step 4 — Screen Edge & Corner Clamping

1. Drag `sample.txt` near the **top-left**, **top-right**, **bottom-left**, and **bottom-right** monitor corners and press `Shift`:
   - Verify the entire 4-ring reserve envelope and the bottom `280×64 lp` caption box remain 100% inside the monitor bounds with no clipping when opening deep submenus (`Convert` → `Preset`).

---

## 5. Regression Suites, Performance Measurements & `TEST_MATRIX.md` Status

### 5.1 Workspace Quality Gate Results

| Gate Command | Result | Details |
|---|---|---|
| `cargo fmt --all --check` | **PASS** | Zero formatting diffs across all 14 workspace crates. |
| `./scripts/check-dep-graph.sh` | **PASS** | `radivine-wheel` depends only on `smallvec`; `radivine-render` depends only on `radivine-wheel`, `fontdue`, `ab_glyph_rasterizer`, `tiny-skia`, `resvg`; `#![forbid(unsafe_code)]` intact in both; `assets/branding/` and `assets/icons/` untouched. |
| `cargo clippy --workspace --all-targets -- -D warnings` | **PASS** | Zero warnings across all library, binary, and test targets. |
| `cargo test --workspace -- --test-threads=1` (under isolated `Xvfb`) | **PASS** | **249 passed, 0 failed, 2 ignored** (long-duration `#[ignore]` 1-hour soak & 10-minute fuzz suites). |

### 5.2 Live `Xvfb` & Benchmark Performance Measurements

> **Important Note on `T-PERF4`:** Per owner instructions, `T-PERF4` is **not** marked `PASS` in Phase A because `T-PERF4` requires a 30-second continuous pointer trace on a real composited desktop (`1×` and `2×` scale) in release mode (`p95 <= 4.0 ms`, `p99 <= 8.0 ms`, `< 1%` dropped frames at `60 Hz`). Below are the measured Phase A figures from `radivine-wheel`, `radivine-render`, and live `Xvfb`.

| Metric / Gate | Target (`TEST_MATRIX.md`) | Measured Value (Phase A) | Profile / Backend | Status |
|---|---|---|---|---|
| **`T-PERF1`** (`radivine-wheel` `hit_test_concentric` + `update` allocs & latency) | `0 B` heap allocs; `p99 < 25 µs` | **`0 B` heap allocs** (`0` allocations across 1,000 sweeps); **`p99 = 0.42 µs`** | Pure engine (`alloc_test` + `concentric_scenarios`) | **PASS** |
| **`T-PERF2`** (`radivine-render` warm cached motion frame allocs) | `0 B` steady-state heap allocs | **`0 B` heap allocs** (`render_alloc_test`) | Pure renderer (`tiny-skia` scratch pixmap + glyph/icon caches) | **PASS** |
| **`T-PERF3`** (`radivine-render` dirty-rect CPU render time, `1×` & `2×`) | `1×`: `p50 <= 1.2 ms`, `p95 <= 2.5 ms`; `2×`: `p50 <= 2.5 ms`, `p95 <= 4.5 ms` | `1×`: **`p50 = 0.41 ms`, `p95 = 0.68 ms`**; `2×`: **`p50 = 1.38 ms`, `p95 = 2.19 ms`** | `radivine-render` (`concentric_render.rs`) | **PASS** |
| **Live `Xvfb` MIT-SHM Damage Present (`x11_damage_present_benchmark_under_2ms`)** | `< 2.0 ms` avg (`600×600` damage rect) | **`0.12 ms` (`120 µs`) avg** | Live `Xvfb` (`radivine-platform`) | **PASS** |
| **Live `Xvfb` Concentric Summon + Initial Frame Present (`frame_render_us`)** | `< 16.6 ms` (`60 Hz` frame budget) | **`2.73`–`2.89 ms` (`2,726`–`2,886 µs`)** | Live `Xvfb` (unoptimized `debug` profile) | Measured (`Xvfb`) |
| **Live `Xvfb` `XdndPosition` → `XdndStatus` Latency (`dnd_status_us`)** | `< 2.0 ms` (`2,000 µs`) | **`0.034`–`0.038 ms` (`34`–`38 µs`)** | Live `Xvfb` (`nested_acceptance` + `m3_drag_acceptance`) | **PASS** |
| **Live `Xvfb` Trigger → Mapped Window Latency (20 trials)** | `p95 < 50 ms` | **`min = 26.02 ms`, `median = 26.24 ms`, `p95 = 26.63 ms`, `max = 26.65 ms`** | Live `Xvfb` (`m3_drag_acceptance`, `debug` profile) | **PASS** |
| **`T-PERF4`** (Complete real X11 composited desktop 30s continuous pointer sweep at `1×` & `2×`) | `p95 <= 4.0 ms`, `p99 <= 8.0 ms`, `< 1%` dropped frames at `60 Hz` | `Xvfb` `dnd_status_us = 34–38 µs`, `frame_render_us = 2.73–2.89 ms`; full 30s real composited `1×`/`2×` trace pending | Live desktop measurement required | **BLOCKED (Real Desktop `1×`/`2×` Trace Pending)** |
| **`T-PERF5`** (200-session Concentric + Branching leak gate) | `< +512 KiB` RSS growth | **`+0 KiB` RSS delta** (`m3_drag_acceptance` 50 live Xvfb drag sessions + `nested_acceptance` 200-session soak) | Live `Xvfb` + `FakePlatform` | **PASS** |
| **`T-PERF6`** (1-hour mixed-session soak `m6_one_hour_isolated_soak_test_ac_6_2` & 1-hour C5 worker soak) | `< 1%` memory drift over 1 hour | Fast smoke gates passed (`m6_soak_smoke_test` 20 cycles `+0 KiB`; `c5_soak_test` 30s `+0 KiB`); 1-hour run deferred per Phase A scope | Long-duration soak | **NOT RUN (Phase B)** |

### 5.3 Complete `TEST_MATRIX.md` Requirement Status Summary

| Category | Test IDs | Phase A Status | Notes |
|---|---|---|---|
| **Geometry & Hit-Testing** | `T-GEO1`–`T-GEO7` | **PASS (`7/7`)** | Including `T-GEO4` (`668×742 lp` minimum viewport, `1366×768@1` Concentric, `1280×720@1` Branching, zero caption/sector clipping). |
| **Navigation & Input State Machine** | `T-SM1`–`T-SM8` | **PASS (`8/8`)** | Verified in `radivine-wheel` (`concentric_scenarios.rs`) and `radivine` (`nested_acceptance.rs`). |
| **Rendering & Visual Quality** | `T-REN1`–`T-REN6` | **PASS (`6/6`)** | Verified in `radivine-render` (`concentric_render.rs`, `render_alloc_test.rs`). |
| **X11 Windowing, Input Shape & XDND** | `T-X111`–`T-X118` | **PASS (`8/8` on Live `Xvfb`)** | All 8 tests executed and passed against real `Xvfb` (`live_xvfb_concentric_end_to_end_xdnd_session_and_compositor_guards` + `m3_real_gtk3_drag_retargeting`). |
| **Configuration, CLI & Settings UI** | `T-CFG1`–`T-CFG6` | **PASS (`6/6`)** | Verified in `nested_config.rs` and `nested_settings.rs`. |
| **Performance, Memory & Stability** | `T-PERF1`, `T-PERF2`, `T-PERF3`, `T-PERF5` | **PASS (`4/6`)** | Verified in `alloc_test`, `render_alloc_test`, `concentric_render`, `m3_drag_acceptance`, and `nested_acceptance`. |
| **Complete Real X11 Presentation Latency** | `T-PERF4` | **BLOCKED (`0/1`)** | Live `Xvfb` present/status latencies measured (`2.73 ms` / `34 µs`); complete 30-second real composited X11 `1×`/`2×` desktop trace still required before marking `PASS`. |
| **Long-Duration Soak & Fuzz Gates** | `T-PERF6` (+ `m6` 10-min fuzz + `c5` 1-hr worker soak) | **NOT RUN (`0/1`)** | Short smoke versions passed (`m6_soak_smoke_test`, `m6_fast_fuzz_smoke_test`, 30s `c5_soak_test`); full 1-hour / 10-minute runs scheduled for final N5 sign-off. |
| **Live Cinnamon / Nemo Desktop Sign-Off** | `T7` (`TEST_MATRIX.md` §7) | **PARTIAL / BLOCKED** | **Owner Manual Positive Smoke Test — PASS** recorded; remaining disposable-file reject-zone & file-identity checklist (§4.2) awaits owner execution. |

---

## 6. Fixes Applied During N5 Phase A

No safety defects were found in production daemon, wheel, renderer, or X11 platform code during live `Xvfb` execution. Four test-harness and report fixes were made and committed:

1. **`docs/test-reports/N4.md` (`d8c8f4b`):** Corrected the misreported `668×668 lp` viewport figure to the actual `668×742 lp` contract enforced by `plan_concentric` (`crates/radivine-wheel/src/concentric.rs:294-324`).
2. **`apps/radivine/tests/nested_acceptance.rs` (`d8c8f4b`):**
   - Added `sha256: [u8; 32]` (`sha2::Sha256`) alongside `path`, `ino`, `size`, `mtime`, and `bytes` in `FileFingerprint`.
   - Added explicit `1366×768@1`, `1280×720@1`, `668×742@1`, `668×741@1`, and `667×742@1` assertions and 4-ring + caption non-clipping assertions.
   - Fixed `live_xvfb_concentric_end_to_end_xdnd_session_and_compositor_guards` to clear inherited `RADIVINE_FORCE_ARGB` on the child daemon (so `_NET_WM_CM_S0` ownership transitions work when the test suite is invoked with `RADIVINE_FORCE_ARGB=1`), fixed CLI `"cancel"` → `"dismiss"`, and expanded the live `Xvfb` test to verify all of `T-X111`–`T-X118` and `T-GEO4`.
3. **`apps/radivine/tests/v0_1_1_login_autostart_regression.rs` (`3bcb025`):** Cleared inherited `RADIVINE_FORCE_ARGB` in `IsolatedDaemon::spawn` so `xvfb_wire_daemon_dynamically_recovers_when_compositor_and_dpi_initialize_after_startup` tests real `_NET_WM_CM_S0` compositor acquisition even when invoked under `RADIVINE_FORCE_ARGB=1`.
4. **`apps/radivine/tests/m2_acceptance.rs` & `apps/radivine/tests/m3_drag_acceptance.rs` (`3bcb025`):** Replaced the fixed `25 ms` sleep after `xdotool` cancel commands in AC-2.4 with the bounded unmap poll (`wait_until_unmapped`, up to `250 ms` in `5 ms` increments) already used in AC-2.5, and widened the debug-mode cold-SVG child-wheel open wait in `m3_real_gtk3_drag_retargeting` so GTK3 receives `XdndStatus(XdndActionPrivate)` before `ButtonRelease`.

---

## 7. Remaining Mandatory N5 Work (Before N6 Authorization)

1. **Owner Disposable-File Desktop Checklist (§4.2):** Execute Steps 0–4 on the live Cinnamon/Nemo desktop to complete `T7` negative/reject-zone, file-identity (`path`, `inode`, `size`, `mtime`, `SHA-256`), settings reload, and corner clamping sign-off.
2. **Complete `T-PERF4` Measurement:** Collect the complete 30-second continuous pointer trace (`1×` and `2×` scaling) on the real X11 desktop in `--release` mode to transition `T-PERF4` from `BLOCKED` to `PASS`.
3. **Long-Duration Soak & Fuzz Runs (`T-PERF6`):**
   - `cargo test --release -p radivine --test m6_soak_test m6_one_hour_isolated_soak_test_ac_6_2 -- --ignored --nocapture` (1 hour)
   - `cargo test --release -p radivine --test m6_fuzz_suite m6_ten_minute_fuzz_suite_ac_6_4 -- --ignored --nocapture` (10 minutes)
   - `RADIVINE_C5_SOAK_SECS=3605 cargo test --release -p radivine-control --test c5_soak_test -- --nocapture` (1 hour)

---

## 8. Git Status & Commits Created in N5 Phase A

| Commit | Description |
|---|---|
| `d8c8f4b` | `test(x11): verify live Xvfb concentric XDND, XShape, and 668x742 viewport gates` |
| `3bcb025` | `fix(test): harden Xvfb timing in m2/m3 acceptance and clear inherited RADIVINE_FORCE_ARGB` |
| `cdb02c7` | `docs(n5): add Milestone N5 Phase A safety and Xvfb verification report` |
| `70f8877` | `test(x11): assert zero overlay subwindows and order-independent root tree in T-X111` |

- **Untracked files preserved untouched:** `.idea/`
- **Installed system state:** Existing `v0.2.0` installation, user configuration, Nemo integration, systemd user services, tags, and releases remain completely untouched.
