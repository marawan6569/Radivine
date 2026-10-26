# Radivine v0.3.0 — Concentric Rings

**Branch into action.**  
*A fast, extensible, context-aware radial action wheel and desktop utility suite for Linux.*

---

## Highlights of v0.3.0

**Radivine v0.3.0** introduces the optional **Concentric (Nested Rings)** radial wheel layout (`wheel.layout = "concentric"`) alongside the preserved default **Branching (Child Wheels)** layout (`wheel.layout = "branching"`), hardened non-actionable X11/XDND drop safety across all nine non-leaf zones, portable Arabic script shaping across the wheel renderer and Control Center, and the complete built-in PNG/JPEG/WebP conversion suite:

### 1. Optional Concentric (Nested Rings) Wheel Layout (`wheel.layout = "concentric"`)
- **Two Selectable Layouts (Branching Default Preserved):** Keep the classic **Branching (Child Wheels)** layout (`wheel.layout = "branching"`, default on new installations and upgrades) or switch to **Concentric (Nested Rings)** (`wheel.layout = "concentric"`) in **Control Center → Wheel → Wheel Layout** (or `~/.config/radivine/config.toml`).
- **Single-Center Multi-Ring Expansion:** Submenus (`File Preview`, `Convert ▸ Image`, `Custom Actions`) promote onto outward concentric rings (`R_hub = 50 lp`, root ring `50..118 lp`, outer ring `L >= 1` at `118 + 66·(L-1)..118 + 66·(L-1) + 60 lp`, inter-ring radial gap `6 lp`, minimum `30°` sector sweep with overflow pagination up to 6 rings) centered on a single immobile root origin (`C_0`).
- **Path-Continuity Markers & Breadcrumb Caption:** Renders angular path-continuity markers linking each open parent sector across the `6 lp` radial gap to its child ring, plus a fixed bottom caption pill (`Parent › Child › Action`) and a displaced-origin approach guide when clamped near screen edges.
- **Zero Warm Heap Allocations:** After cache warmup, Concentric hover and ring-replacement frames allocate `0` heap bytes (`~4.85 µs` `p50` engine hit-test+tick+scene across 6 levels; `~171 µs` `p50` warm 1080p render).
- **Deferred Active-Session Reload:** Changing `wheel.layout` in Control Center or via `radivine reload` while a wheel session is open preserves the active session's snapshotted layout (`layout_pending = true`) and applies the new layout on the next summon.

### 2. Reserved `668 × 742` Viewport Contract & Automatic Small-Screen Fallback
- **4-Ring + Fixed Caption Reservation:** Before mapping the overlay, Concentric mode reserves 4 rings (`R_reserve(4) = 326 lp`, `±334 lp` horizontal/top with `8 lp` screen margin) plus the fixed bottom caption box (`Cy + 326..388 lp`, `+408 lp` bottom extent with `20 lp` margin), requiring a minimum monitor workarea of **`668 × 742` logical pixels**.
- **Automatic Session Fallback:** Standard laptop and desktop viewports (`1366×768@1x`, `1920×1080@1x`, `1920×1080@1.25x`, `3840×2160@2x`) support Concentric mode with zero clipping. Smaller logical viewports (`1280×720@1x`, `1920×1080@1.5x`) automatically fall back to `Branching` (`layout_fallback_reason = "insufficient_space"`) for that session without modifying `~/.config/radivine/config.toml`.

### 3. Strict Non-Actionable Drop & Click Safety Envelope (`T-SAFE1`–`T-SAFE10`)
- **Nine Non-Actionable Reject Zones:** Releasing a file drag or clicking in the central hub (`r < 50 lp`), inter-ring radial gaps (`6 lp`), angular separator gaps, outer ring margin (`r > R_out(D)`), fixed bottom caption box, displaced-center approach corridor (`> 4 lp` anchor offset), protected pending-switch sibling sectors (`90 ms` outer-ring steering protection), submenu parent sectors (on drag release), or outside the cluster reports `XdndStatus(accept=0)` and finishes with `XdndFinished(success=0)`—never executing an unintended action or dropping files into underlying Nemo windows (`input.passthrough = false` default).
- **Drop-Time Coordinate Re-Evaluation:** `XdndDrop` re-evaluates the final pointer position before committing and never executes a stale hovered action.

### 4. Built-In Image Conversion Suite & Durable Background Worker (`Convert ▸ Image`)
- **One-Gesture Quick Convert & Advanced Convert Dialog:** Convert single or batch images between **PNG**, **JPEG**, and **WebP** directly from the radial wheel (`Convert ▸ Image ▸ PNG / JPEG / WebP`) or open the dedicated **Advanced Convert** window (`radivine-control convert`) with 5 built-in presets (`Web Optimized`, `Balanced`, `High Quality`, `Lossless`, `Compact Share`), custom presets (`~/.config/radivine/convert.toml`), Lanczos3 resize, EXIF auto-orientation, ICC profile preservation, and privacy-by-default EXIF stripping.
- **Durable Out-of-Process Worker (`radivine-worker`):** Runs conversions in a background worker backed by SQLite WAL (`~/.local/state/radivine/jobs.sqlite3`) with `O_RDONLY | O_NOFOLLOW` source validation, `.rvstage-<uid>` staging, atomic `renameat2(RENAME_NOREPLACE)` non-overwriting file commits, cooperative cancellation, linked retries, and automatic `60 s` idle exit.

### 5. Portable Arabic Font Fallback & Debian Runtime Dependencies
- **Arabic Script Shaping in Wheel & UI:** `radivine-render` shapes Arabic filenames and custom action labels via system `DejaVu Sans` (`DejaVuSans.ttf` / `DejaVuSans-Bold.ttf`) while keeping bundled `Inter-Variable.ttf` isolated for deterministic Latin typography; `radivine-control` and `radivine-viewer` use Fontconfig (`egui_system_font_provider` + `harfrust`) for contextual Arabic glyph joining and Lam-Alef (`لا`) ligatures.
- **Explicit Debian Package Dependencies:** `radivine_0.3.0_amd64.deb` declares `Depends: libc6 (>= 2.34), libgcc-s1 (>= 3.0), fonts-dejavu-core, libfontconfig1` and `Recommends: nemo, libx11-6, libegl1, libgl1, systemd, fonts-noto-core`.

---

## Supported Environments & Installation

- **Primary Target Desktop:** **Linux Mint 22.3 Cinnamon (X11, `amd64`)**
- **Compatible Base:** **Ubuntu 24.04 LTS+** (`amd64`, `glibc >= 2.34`) and other X11 desktops (XFCE, MATE, GNOME on Xorg, KDE Plasma X11)
- **Package Filename:** `radivine_0.3.0_amd64.deb` (`16,149,118` bytes / `15.40 MiB`)
- **Direct `.deb` Download URL:**
  https://github.com/marawan6569/Radivine/releases/download/v0.3.0/radivine_0.3.0_amd64.deb
- **SHA-256 Checksum:**
  ```text
  4c0b8cc719963dec2f2457588a500a3530dd4aa88b01d0b3954eeb57edb37156  radivine_0.3.0_amd64.deb
  ```

### Quick Install on Linux Mint 22.3 Cinnamon / Ubuntu 24.04+

```bash
# 1. Download the stable .deb package and SHA256SUMS.txt
curl -fLO https://github.com/marawan6569/Radivine/releases/download/v0.3.0/radivine_0.3.0_amd64.deb
curl -fLO https://github.com/marawan6569/Radivine/releases/download/v0.3.0/SHA256SUMS.txt

# 2. Verify the SHA-256 checksum
sha256sum -c SHA256SUMS.txt

# 3. Install the package via apt (installs radivine 0.3.0 + fonts-dejavu-core + libfontconfig1)
sudo apt install -y ./radivine_0.3.0_amd64.deb

# 4. Start (or restart) the background radial wheel service and open Radivine Control Center
systemctl --user daemon-reload
systemctl --user enable --now radivine.service
systemctl --user restart radivine.service
radivine-control
```

---

## Upgrading from v0.2.0 or v0.3.0-rc.1 & Rollback

### Upgrading (`v0.2.0 -> 0.3.0` or `0.3.0~rc1 -> 0.3.0`)
- Installing `./radivine_0.3.0_amd64.deb` over `v0.2.0` or `0.3.0~rc1` preserves `~/.config/radivine/config.toml`, `~/.config/radivine/actions.toml`, `~/.config/radivine/convert.toml`, and `~/.local/state/radivine/jobs.sqlite3` byte-for-byte (`0.2.0 < 0.3.0~rc1 < 0.3.0` in `dpkg`).
- Existing configurations without `wheel.layout` remain on `"branching"` until changed in **Control Center → Wheel → Wheel Layout**.

### Rolling Back Layout or Package Version
- **Instant Layout Rollback (No Reinstall Required):** Set **Wheel Layout** to **`Branching (Child Wheels)`** in `radivine-control` (or set `layout = "branching"` under `[wheel]` in `~/.config/radivine/config.toml` and run `radivine reload`).
- **Full Package Downgrade to `v0.2.0`:**
  ```bash
  curl -fLO https://github.com/marawan6569/Radivine/releases/download/v0.2.0/radivine_0.2.0_amd64.deb
  sudo apt install -y --allow-downgrades ./radivine_0.2.0_amd64.deb
  systemctl --user daemon-reload
  systemctl --user restart radivine.service
  ```

---

## Known Limitations

- **X11 Display Server Required for Radial Wheel Overlay:** Global `Drag + Shift` (`XDND`) interception and pointer-tracking overlay require an **X11 session** (`XDG_SESSION_TYPE=x11`). Under Wayland, `radivine doctor` and the Control Center report session details and recommend switching to an X11 session for mid-drag wheel interception (`radivine invoke`, `radivine-control`, `radivine-viewer`, and `radivine-worker` remain operable).
- **Minimum Viewport for Concentric Layout (`668 × 742` Logical Pixels):** Concentric mode reserves 4 rings plus the bottom caption pill (`668 × 742` logical px). Monitors with smaller logical workareas (such as `1280×720@1x` or `1920×1080@1.5x`) automatically fall back to `Branching` for that session.
- **Static Raster Images Only (`PNG`, `JPEG`, `WebP`):** Built-in conversion supports static `PNG`, `JPEG`, and `WebP` images (`<= 16,384 × 16,384` px, `<= 100 MP`). Animated GIF/APNG/WebP, HEIC/AVIF, SVG, PDF, and audio/video transcoding are out of scope for `v0.3.0`.
- **Left-to-Right Widget Container Alignment (`egui 0.37`):** While `cosmic-text` / `harfrust` performs contextual Arabic glyph shaping and inline bidirectional run reordering, `egui 0.37` widget container layout alignment remains LTR.
- **System Tray Indicator Deferred (`ADR-025`):** Daemon lifecycle is managed directly via the **Radivine Control Center** Dashboard and `systemd --user` rather than a persistent GTK/AppIndicator panel tray icon.

---

## License

Dual-licensed under **`MIT OR Apache-2.0`**. See [`LICENSE-MIT`](https://github.com/marawan6569/Radivine/blob/main/LICENSE-MIT), [`LICENSE-APACHE`](https://github.com/marawan6569/Radivine/blob/main/LICENSE-APACHE), and [`THIRD_PARTY_NOTICES.md`](https://github.com/marawan6569/Radivine/blob/main/THIRD_PARTY_NOTICES.md).
